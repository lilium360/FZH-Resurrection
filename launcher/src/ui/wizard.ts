import { errorMessage, type DumpKind } from "../backend";
import { t, th } from "../i18n";
import { attempt, cancelInstall, changeLocation, chooseFolder, chooseServer, install, prepare, register, signedIn } from "./actions";
import { esc, gb, lab, options, planText, progressBar, recognised, rejection, trh } from "./components";
import { I } from "./icons";
import { WIZARD_LAST, fileName, wizardSteps } from "./meta";
import { modalEl, render, toast } from "./overlay";
import { DP, F, PR, REC, S, SAVED, app, backend, canPlay, installing, missing, pkgChosen, srvName } from "./state";

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

/** What is typed in the Account step, so that a redraw keeps it; null: not typed yet. */
const TYPED = { id: null as string | null, code: "", server: null as string | null };
/** The Account step shows the private server's address field instead of the account's. */
let choosingServer = false;

export function wzTyped(k: string, v: string): void {
  if (k in TYPED) (TYPED as Record<string, string | null>)[k] = v;
}

/** The Online ID stays for a later visit of the step; the recovery code does not wait in memory. */
function leaveAccount(): void {
  TYPED.code = "";
  TYPED.server = null;
  choosingServer = false;
}

function forgetTyped(): void {
  Object.assign(TYPED, { id: null, code: "", server: null });
  choosingServer = false;
}

const wzField = (label: string, k: string, value: string, placeholder = ""): string =>
  `<label>${th(label)}<span class="fld w"><input data-wzf="${k}" value="${esc(value)}" spellcheck="false" autocomplete="off" placeholder="${placeholder}"></span></label>`;

const wzLink = (a: string, label: string): string => `<button class="b-link" data-wz="${a}">${th(label)}</button>`;

/** Which server the account is for: one line under the text, since most players stay on the community server. */
function serverLine(): string {
  return SAVED.usePrivateServer
    ? `<p class="b-note">${th("wizard.step3.onPrivate")} ${wzLink("server-edit", "wizard.step3.changeAddress")} · ${wzLink("server-official", "wizard.step3.useOfficial")}</p>`
    : `<p class="b-note">${th("wizard.step3.privateQuestion")} ${wzLink("server-edit", "wizard.step3.setPrivate")}</p>`;
}

function account(): string {
  if (PR.in) return `<div class="b-wbody">${rowW(true, th("wizard.step3.signedIn", { id: PR.id }), trh(srvName()))}</div>`;
  const acts = (btns: string) => `<div class="b-acts" style="justify-content:flex-start;margin-top:4px">${btns}</div>`;
  if (choosingServer)
    return `<div class="b-form b-wbody">${wzField("meta.privateServer.label", "server", TYPED.server ?? SAVED.privateServer)}<p class="b-note">${th("meta.privateServer.note")}</p>${acts(
      `<button class="btn fill" data-wz="server-save">${th("wizard.step3.useServer")}</button><button class="btn" data-wz="server-cancel">${th("dialog.unsavedChanges.cancel")}</button>`,
    )}</div>`;
  return `<div class="b-form b-wbody">${serverLine()}${wzField("wizard.step3.onlineId", "id", TYPED.id ?? DP.id)}${wzField("wizard.step3.recoveryCode", "code", TYPED.code, th("wizard.step3.recoveryPlaceholder"))}${acts(
    `<button class="btn fill" data-wz="login">${th("wizard.step3.signIn")}</button><button class="btn" data-wz="create">${th("wizard.step3.createAccount")}</button>`,
  )}</div>`;
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
  const active = document.activeElement as HTMLInputElement | null;
  const focused = active && modalEl.contains(active) ? active.dataset.wzf : undefined;
  const caret = active?.selectionStart ?? null;
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
  const again = focused ? modalEl.querySelector<HTMLInputElement>(`[data-wzf="${focused}"]`) : null;
  if (!again) return;
  again.focus();
  again.setSelectionRange(caret, caret);
}

function finish(message: string): void {
  app.wz = null;
  forgetTyped();
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
    leaveAccount();
  } else if (a === "back") {
    app.wz--;
    leaveAccount();
  } else if (a === "skip") {
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
      if (await register(id)) forgetTyped();
      return;
    }
    if (!code) {
      toast(t("toast.enterRecoveryCode"), true);
      return;
    }
    try {
      await signedIn(await backend.signIn(id, code));
      forgetTyped();
      toast(t("toast.signedInAs", { id: PR.id }));
    } catch (e) {
      toast(errorMessage(e), true);
      return;
    }
  } else if (a === "server-edit" || a === "server-cancel") {
    choosingServer = a === "server-edit";
    TYPED.server = null;
  } else if (a === "server-save" || a === "server-official") {
    if (!(await chooseServer(a === "server-save", a === "server-save" ? field("server") : undefined))) return;
    choosingServer = false;
    TYPED.server = null;
    toast(t("toast.changesSaved"));
  }
  wizard();
  render();
  if (a === "server-edit") modalEl.querySelector<HTMLInputElement>('[data-wzf="server"]')?.select();
}

