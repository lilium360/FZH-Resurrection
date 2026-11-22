import { errorMessage, type DumpKind } from "../backend";
import { t, th } from "../i18n";
import { attempt, cancelInstall, changeLocation, chooseFolder, install, prepare, register, signedIn } from "./actions";
import { esc, gb, lab, options, planText, progressBar, recognised, rejection, trh } from "./components";
import { I } from "./icons";
import { WIZARD_LAST, fileName, wizardSteps } from "./meta";
import { modalEl, render, toast } from "./overlay";
import { DP, F, PR, REC, S, app, backend, canPlay, installing, missing, pkgChosen, srvName } from "./state";

const okI = (b: boolean) => `<span class="ic ${b ? "ok" : "idle"}">${I(b ? "check" : "clock", 13)}</span>`;
const rowW = (ok: boolean, title: string, d: string, btn = "") => `<div class="b-chk">${okI(ok)}<div><b>${title}</b><small>${d}</small></div>${btn}</div>`;
const rowBad = (title: string, d: string, btn: string) =>
  `<div class="b-chk"><span class="ic warn">${I("alert", 13)}</span><div><b>${title}</b><small class="warn">${d}</small></div>${btn}</div>`;

function needRow(need: DumpKind, busy: boolean): string {
  const btn = `<button class="btn sm fill" data-wz="${need}" ${busy ? "disabled" : ""}>${th("screen.play.chooseFolder")}</button>`;
  const name = esc(fileName(need));
  if (F.err) return rowBad(name, rejection(), btn);
  if (F.note) return rowBad(name, trh(F.note), btn);
  return rowW(false, name, th(need === "update" ? "wizard.step1.needUpdate" : "wizard.step1.needBase"), btn);
}

function installRow(need: boolean): string {
  if (!pkgChosen()) return "";
  const label = th("wizard.step1.installLabel");
  if (F.inst?.complete) return rowW(true, label, th("wizard.step1.installDone", { size: gb(F.inst.size) }));
  if (installing())
    return `<div class="b-chk">${okI(false)}<div><b>${label}</b><small>${progressBar()}</small></div><button class="btn sm" data-wz="cancel-install">${th("install.cancel")}</button></div>`;
  if (need) return rowW(false, label, th("wizard.step1.installWait"));
  const blocked = !F.plan || !!F.plan.error || app.run || F.prep === 1;
  const btns = `<span class="b-btns"><button class="btn sm" data-wz="location" ${app.run ? "disabled" : ""}>${th("install.change")}</button><button class="btn sm fill" data-wz="install" ${blocked ? "disabled" : ""}>${th(F.plan?.present ? "wizard.step1.installResume" : "wizard.step1.installStart")}</button></span>`;
  const text = `${th("install.installLocation")}: <code>${esc(F.plan?.target ?? F.inst?.target ?? "")}</code><br>${planText()}${F.instErr ? `<br><span class="bad">${trh(F.instErr)}</span>` : ""}`;
  return `<div class="b-chk">${okI(false)}<div><b>${label}</b><small>${text}</small></div>${btns}</div>`;
}

/** A language picker that saves at once (`onModalChange` in events.ts). */
const picker = (k: "launcherLanguage" | "gameLanguage", labelKey: string): string =>
  `<div class="b-form b-wbody"><label>${th(labelKey)}<select class="sel" data-wzk="${k}">${options(k)
    .map((v) => `<option value="${esc(v)}" ${S[k] === v ? "selected" : ""}>${esc(lab(v, k))}</option>`)
    .join("")}</select></label></div>`;

function gameFiles(): string {
  const need = missing();
  const busy = F.prep === 1 || app.run || installing();
  const any = !!F.base || !!F.update;
  const gameBtn = `<button class="btn sm" data-wz="game" ${busy ? "disabled" : ""}>${th(any ? "install.change" : "screen.play.chooseFolder")}</button>`;
  const folder = esc(fileName("game"));
  const prep = th("wizard.step1.preparation");
  return `<div class="b-wbody">${
    F.err && !need ? rowBad(folder, rejection(), gameBtn) : rowW(any, folder, any ? esc(recognised()) : th("gameSteps.noFolderChosen"), gameBtn)
  }${need ? needRow(need, busy) : ""}${installRow(!!need)}${
    F.prepErr
      ? rowBad(prep, trh(F.prepErr), `<button class="btn sm" data-wz="prepare" ${busy ? "disabled" : ""}>${th("gameSteps.tryAgain")}</button>`)
      : rowW(F.prep === 2, prep, th(F.prep === 2 ? "wizard.step1.prepDone" : F.prep === 1 ? "wizard.step1.preparing" : "wizard.step1.prepAuto"))
  }</div>`;
}

function account(): string {
  if (PR.in) return `<div class="b-wbody">${rowW(true, th("wizard.step3.signedIn", { id: PR.id }), trh(srvName()))}</div>`;
  return `<div class="b-form b-wbody"><label>${th("wizard.step3.onlineId")}<span class="fld w"><input data-wzf="id" value="${esc(DP.id)}" spellcheck="false" autocomplete="off"></span></label><label>${th("wizard.step3.recoveryCode")}<span class="fld w"><input data-wzf="code" spellcheck="false" autocomplete="off" placeholder="${th("wizard.step3.recoveryPlaceholder")}"></span></label><div class="b-acts" style="justify-content:flex-start;margin-top:4px"><button class="btn fill" data-wz="login">${th("wizard.step3.signIn")}</button><button class="btn" data-wz="create">${th("wizard.step3.createAccount")}</button></div></div>`;
}

function ready(): string {
  return `<div class="b-wbody">${rowW(F.prep === 2, th("wizard.step4.filesLabel"), th(F.prep === 2 ? "wizard.step4.filesReady" : "wizard.step4.filesNotReady"))}${rowW(
    PR.in,
    th("wizard.step4.accountLabel"),
    PR.in ? th("wizard.step4.accountReady", { id: PR.id }) : th("wizard.step4.accountNotReady"),
  )}</div>`;
}

/** Shown once after a registration, above the assistant when it is open; Continue waits for "I saved it". */
function recoveryCode(): void {
  modalEl.innerHTML = `<div class="b-modal"><div class="b-dlg" role="dialog" aria-modal="true" aria-labelledby="rc-t"><h2 id="rc-t">${th("dialog.recoveryCode.title")}</h2><p>${th("dialog.recoveryCode.body")}</p>
    <div class="b-code" data-rec-code>${esc(REC.code)}</div>
    <div class="b-acts" style="justify-content:flex-start"><button class="btn" data-rec="copy">${th("dialog.recoveryCode.copy")}</button></div>
    <button class="ck" data-rc="saved" role="checkbox" aria-checked="false"><u>${I("check", 11)}</u>${th("dialog.recoveryCode.saved")}</button>
    <div class="b-acts"><button class="btn fill" data-rec="done" disabled>${th("dialog.recoveryCode.continue")}</button></div></div></div>`;
}

/** The five steps: launcher language, game files, game language, account, ready. */
export function wizard(): void {
  if (REC.code) {
    recoveryCode();
    return;
  }
  const WZ = app.wz;
  if (WZ === null) {
    modalEl.innerHTML = "";
    return;
  }
  const steps = wizardSteps();
  const body = [
    () => picker("launcherLanguage", "wizard.step0.label"),
    gameFiles,
    () => picker("gameLanguage", "wizard.step2.label"),
    account,
    ready,
  ][WZ]?.() ?? "";
  const text =
    WZ === 3
      ? th("wizard.step3.body", { server: srvName() })
      : WZ === WIZARD_LAST
        ? th(canPlay() ? "wizard.step4.bodyReady" : "wizard.step4.bodyNotReady")
        : th(`wizard.step${WZ}.body`);
  const next = th(WZ === 0 ? "wizard.nav.start" : WZ === WIZARD_LAST ? "wizard.nav.open" : "wizard.nav.next");
  modalEl.innerHTML = `<div class="b-modal"><div class="b-dlg wide" role="dialog" aria-modal="true" aria-labelledby="wz-t"><div class="b-wsteps">${steps
    .map((s, i) => `<i class="${i <= WZ ? "on" : ""}" title="${esc(s)}"></i>`)
    .join("")}</div><h2 id="wz-t">${th(`wizard.step${WZ}.title`)}</h2><p>${text}</p>${body}
    <div class="b-acts split">${WZ < WIZARD_LAST ? `<button class="b-link" data-wz="skip">${th("wizard.nav.skip")}</button>` : "<span></span>"}<span>${WZ > 0 ? `<button class="btn" data-wz="back">${th("wizard.nav.back")}</button>` : ""}<button class="btn fill" data-wz="next">${next}</button></span></div></div></div>`;
}

function finish(message: string): void {
  app.wz = null;
  wizard();
  render();
  toast(message);
  void attempt(() => backend.setSetupCompleted(true));
}

const field = (k: string): string => modalEl.querySelector<HTMLInputElement>(`[data-wzf="${k}"]`)?.value ?? "";

export async function wzAct(a: string): Promise<void> {
  if (app.wz === null) return;
  if (a === "next") {
    if (app.wz === WIZARD_LAST) {
      finish(t("wizard.toast.finished"));
      return;
    }
    app.wz++;
  } else if (a === "back") app.wz--;
  else if (a === "skip") {
    finish(t("wizard.toast.skipped"));
    return;
  } else if (a === "game" || a === "base" || a === "update") {
    await chooseFolder(a);
    return;
  } else if (a === "prepare") {
    await prepare();
    return;
  } else if (a === "location") {
    await changeLocation();
    return;
  } else if (a === "install") {
    await install();
    return;
  } else if (a === "cancel-install") {
    await cancelInstall();
    return;
  } else if (a === "login" || a === "create") {
    const id = field("id").trim(),
      code = field("code").trim();
    if (!id) {
      toast(t("toast.emptyOnlineId"), true);
      return;
    }
    if (a === "create") {
      await register(id);
      return;
    }
    if (!code) {
      toast(t("toast.enterRecoveryCode"), true);
      return;
    }
    try {
      await signedIn(await backend.signIn(id, code));
      toast(t("toast.signedInAs", { id: PR.id }));
    } catch (e) {
      toast(errorMessage(e), true);
      return;
    }
  }
  wizard();
  render();
}

