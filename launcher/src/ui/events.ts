import { errorMessage, type ExternalLink, type PickKind } from "../backend";
import { setLang, t, th, tr } from "../i18n";
import {
  attempt,
  cancelInstall,
  changeLocation,
  chooseFolder,
  deleteInstall,
  discard,
  install,
  checkUpdate,
  downloadUpdate,
  go,
  launch,
  prepare,
  recoverySaved,
  register,
  restartToUpdate,
  save,
  signOut,
  signedIn,
  stopGame,
} from "./actions";
import { picturePng } from "./avatar";
import { esc, fmt, gb } from "./components";
import { I } from "./icons";
import { GRIP, M, MENU, type ScreenId, type SettingKey } from "./meta";
import { MODAL, ask, closeModal, confirmIt, modalEl, render, root, toast } from "./overlay";
import {
  CACHE,
  CHECK,
  DP,
  F,
  PR,
  REC,
  S,
  SAVED,
  UPD,
  app,
  applyFiles,
  backend,
  dirtyP,
  dirtyS,
  setSetting,
  srvName,
} from "./state";
import { wizard, wzAct, wzTyped } from "./wizard";

const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;

/** `l` and `ph` are i18n keys. */
const fld = (l: string, k: string, type = "text", ph = "") =>
  `<label>${th(l)}<span class="fld w"><input type="${type}" data-f="${k}" autocomplete="off" spellcheck="false" placeholder="${ph ? th(ph) : ""}"></span></label>`;
const val = (el: HTMLElement, k: string) => el.querySelector<HTMLInputElement>(`[data-f="${k}"]`)?.value ?? "";
const ck = (k: string, l: string, on = true) =>
  `<button class="ck ${on ? "on" : ""}" data-rc="${k}" role="checkbox" aria-checked="${on}"><u>${I("check", 11)}</u>${th(l)}</button>`;

function isKey(k: string | undefined): k is SettingKey {
  return !!k && k in S;
}

async function onClick(e: MouseEvent): Promise<void> {
  const target = e.target as HTMLElement;
  const q = (s: string) => target.closest<HTMLButtonElement>(s);
  let b: HTMLButtonElement | null;

  if ((b = q("[data-go]"))) {
    go(b.dataset.go as ScreenId);
    return;
  }
  if ((b = q("[data-choose]"))) {
    if (b.disabled) return;
    await chooseFolder(b.dataset.choose as Exclude<PickKind, "install">);
    return;
  }
  if ((b = q("[data-files]"))) {
    if (b.disabled) return;
    const kept = F.inst?.complete ? th("dialog.changeFolder.bodyInstall", { target: F.inst.target, size: gb(F.inst.size) }) : "";
    confirmIt(
      th("dialog.changeFolder.title"),
      th("dialog.changeFolder.body") + kept,
      th("dialog.changeFolder.confirm"),
      async () => {
        const files = await attempt(() => backend.resetGameFiles());
        if (!files) return;
        applyFiles(files);
        Object.assign(F, { err: null, found: null, note: null, prepErr: null, plan: null, planErr: null, instErr: null });
        toast(t("toast.chooseAgain"));
      },
      false,
    );
    return;
  }
  if ((b = q("[data-install]"))) {
    if (b.disabled) return;
    const a = b.dataset.install;
    if (a === "location") await changeLocation();
    else if (a === "start") await install();
    else if (a === "cancel") await cancelInstall();
    return;
  }
  if ((b = q("[data-uninstall]"))) {
    if (b.disabled) return;
    deleteInstall(b.dataset.uninstall ?? "", Number(b.dataset.size ?? 0));
    return;
  }
  if ((b = q("[data-prepare]"))) {
    if (b.disabled) return;
    await prepare();
    return;
  }
  if ((b = q('[data-act="play"]'))) {
    if (b.disabled) return;
    if (app.run) {
      confirmIt(th("dialog.stopGame.title"), th("dialog.stopGame.body"), th("dialog.stopGame.confirm"), async () => {
        await attempt(async () => {
          await stopGame();
          toast(t("toast.gameClosed"));
        });
      });
      return;
    }
    if (dirtyS()) toast(t("toast.unsavedNotUsed"), true);
    await launch();
    return;
  } else if ((b = q("[data-copy]"))) {
    try {
      await navigator.clipboard.writeText(app.log.map((l) => `${l[0]} ${tr(l[1])}`).join("\n"));
    } catch {
      toast(t("toast.logCopyFailed"), true);
      return;
    }
    toast(t("toast.logCopied"));
    return;
  } else if ((b = q("[data-clear]"))) {
    if (!app.log.length) return;
    app.log = [];
    toast(t("toast.logCleared"));
  } else if ((b = q("[data-report]"))) {
    ask(
      th("dialog.report.title"),
      th("dialog.report.body"),
      [
        { label: th("dialog.report.cancel") },
        {
          label: th("dialog.report.confirm"),
          cls: "fill",
          fn: async (el) => {
            const description = el.querySelector("textarea")?.value.trim() ?? "";
            if (!description) {
              toast(t("toast.describeFirst"), true);
              return false;
            }
            const on = (k: string) => !!el.querySelector(`[data-rc="${k}"].on`);
            const path = await attempt(() =>
              backend.saveProblemReport({ description, attachLogs: on("logs"), attachSystem: on("sys") }, t("dialog.saveReportPicker")),
            );
            if (!path) return false;
            ask(th("dialog.reportSaved.title"), th("dialog.reportSaved.body", { filename: path.split(/[\\/]/).pop() ?? path }), [
              { label: th("dialog.reportSaved.close") },
              { label: th("dialog.report.openIssues"), cls: "fill", fn: () => attempt(() => backend.openLink("issues")) },
            ]);
            return true;
          },
        },
      ],
      `<div class="b-form b-wbody"><label>${th("dialog.report.whatHappened")}<span class="fld ta"><textarea placeholder="${th("dialog.report.placeholder")}"></textarea></span></label>${ck("logs", "dialog.report.attachLogs")}${ck("sys", "dialog.report.attachSystem")}<p class="b-note">${th("dialog.report.issueNote")} <button class="b-link" data-link="issues">${th("dialog.report.openIssues")}</button></p></div>`,
      true,
    );
    return;
  } else if ((b = q("[data-save]"))) {
    await save();
    return;
  } else if ((b = q("[data-discard]"))) {
    confirmIt(th("dialog.discardChanges.title"), th("dialog.discardChanges.body"), th("dialog.discardChanges.confirm"), () => {
      discard();
      toast(t("toast.changesDiscarded"));
    });
    return;
  } else if ((b = q("[data-host]"))) {
    if (b.disabled) return;
    if (app.host) {
      confirmIt(th("dialog.stopHosting.title"), th("dialog.stopHosting.body"), th("dialog.stopHosting.confirm"), async () => {
        const h = await attempt(() => backend.stopHosting());
        if (!h) return;
        app.host = h.running;
        toast(t("toast.hostingStopped"));
      });
      return;
    }
    if (!SAVED.usePrivateServer) {
      toast(t("toast.savePrivateFirst"), true);
      return;
    }
    const h = await attempt(() => backend.startHosting());
    if (!h) return;
    app.host = h.running;
    toast(t("toast.hostingStarted"));
  } else if ((b = q("[data-upd]"))) {
    const a = b.dataset.upd;
    if (a === "hide") {
      UPD.hidden = true;
      toast(t("toast.updateHidden"));
    } else if (a === "check") {
      if (!b.disabled) await checkUpdate(false);
      return;
    } else if (a === "notes") {
      ask(
        th("dialog.updateNotes.title", { version: UPD.next }),
        th("dialog.updateNotes.body", { current: UPD.cur }),
        [
          { label: th("dialog.updateNotes.later") },
          { label: th("dialog.updateNotes.confirm"), cls: "fill", fn: () => void setTimeout(() => void downloadUpdate()) },
        ],
        `<ul>${UPD.notes.map((n) => `<li>${esc(n)}</li>`).join("")}</ul>`,
      );
      return;
    } else if (a === "download") {
      if (!b.disabled) await downloadUpdate();
      return;
    } else if (a === "restart") {
      if (!b.disabled) restartToUpdate();
      return;
    }
  } else if ((b = q("[data-src]"))) {
    await attempt(() => backend.openLink("source"));
    return;
  } else if ((b = q("[data-gpl]"))) {
    await attempt(() => backend.openLink("licence"));
    return;
  } else if ((b = q("[data-lic]"))) {
    await attempt(() => backend.openLink("licences"));
    return;
  } else if ((b = q("[data-wizard]"))) {
    app.wz = 0;
    wizard();
    return;
  } else if ((b = q("[data-k][data-v]"))) {
    if (isKey(b.dataset.k)) setSetting(b.dataset.k, b.dataset.v);
  } else if ((b = q("[data-t]"))) {
    const k = b.dataset.t;
    if (isKey(k)) setSetting(k, !S[k]);
  } else if ((b = q("[data-grip]"))) {
    if (b.disabled) return;
    Object.assign(S, GRIP);
    toast(t("toast.calibrationReset"));
  } else if ((b = q("[data-check]"))) {
    const c = await attempt(() => backend.getControllerCheck());
    if (!c) return;
    Object.assign(CHECK, c);
    toast(t("toast.checkRefreshed"));
  } else if ((b = q("[data-cache]"))) {
    if (b.disabled) return;
    if (b.dataset.cache === "warm") {
      const c = await attempt(() => backend.startShaderWarmup());
      if (!c) return;
      Object.assign(CACHE, c);
      toast(t("toast.warmupStarted"));
    } else if (b.dataset.cache === "cancel") {
      confirmIt(th("dialog.cancelWarmup.title"), th("dialog.cancelWarmup.body"), th("dialog.cancelWarmup.confirm"), async () => {
        const c = await attempt(() => backend.cancelShaderWarmup());
        if (!c) return;
        Object.assign(CACHE, c);
        toast(t("toast.warmupCancelled"));
      });
      return;
    } else {
      confirmIt(th("dialog.clearCache.title"), th("dialog.clearCache.body"), th("dialog.clearCache.confirm"), async () => {
        const c = await attempt(() => backend.clearShaderCache());
        if (!c) return;
        Object.assign(CACHE, c);
        toast(t("toast.cacheCleared"));
      });
      return;
    }
  } else if ((b = q("[data-login]"))) {
    const id = DP.id.trim();
    if (!id) {
      toast(t("toast.emptyOnlineId"), true);
      return;
    }
    const code = root.querySelector<HTMLInputElement>("[data-code]")?.value.trim() ?? "";
    if (!code) {
      toast(t("toast.enterRecoveryCode"), true);
      return;
    }
    try {
      await signedIn(await backend.signIn(id, code));
      toast(t("toast.signedInAs", { id: PR.id }));
    } catch (err) {
      toast(errorMessage(err), true);
      return;
    }
  } else if ((b = q("[data-signup]"))) {
    ask(
      th("dialog.createAccount.title"),
      th("dialog.createAccount.body", { server: srvName() }),
      [
        { label: th("dialog.createAccount.cancel") },
        {
          label: th("dialog.createAccount.confirm"),
          cls: "fill",
          fn: async (el) => {
            const id = val(el, "id").trim();
            if (!id) {
              toast(t("backend.enter_online_id"), true);
              return false;
            }
            return register(id);
          },
        },
      ],
      `<div class="b-form b-wbody">${fld("dialog.createAccount.onlineId", "id")}</div>`,
    );
    return;
  } else if ((b = q("[data-email]"))) {
    if (b.disabled) return;
    ask(
      th(PR.email ? "dialog.recoveryEmail.titleChange" : "dialog.recoveryEmail.titleAdd"),
      th("dialog.recoveryEmail.body"),
      [
        { label: th("dialog.recoveryEmail.cancel") },
        {
          label: th("dialog.recoveryEmail.confirm"),
          cls: "fill",
          fn: async (el) => {
            const m = val(el, "mail").trim();
            if (!EMAIL.test(m)) {
              toast(t("toast.checkEmail"), true);
              return false;
            }
            const s = await attempt(() => backend.setRecoveryEmail(m));
            if (!s) return false;
            PR.email = s.email;
            toast(t("toast.recoveryEmailSaved"));
            return true;
          },
        },
      ],
      `<div class="b-form b-wbody">${fld("dialog.recoveryEmail.label", "mail", "email")}</div>`,
    );
    return;
  } else if ((b = q("[data-logout]"))) {
    confirmIt(
      th("dialog.signOut.title"),
      (app.run ? th("dialog.signOut.bodyGame") : "") + th(dirtyP() ? "dialog.signOut.bodyUnsaved" : "dialog.signOut.body"),
      th("dialog.signOut.confirm"),
      async () => {
        await signOut();
        toast(t("toast.signedOut"));
      },
    );
    return;
  } else if ((b = q("[data-upload]"))) {
    e.preventDefault();
    if (b.disabled) return;
    document.getElementById("av-file")?.click();
    return;
  } else if ((b = q("[data-avdel]"))) {
    e.preventDefault();
    confirmIt(th("dialog.removeAvatar.title"), th("dialog.removeAvatar.body"), th("dialog.removeAvatar.confirm"), () => {
      DP.img = null;
      if (DP.av === "img") DP.av = 0;
      toast(t("toast.imageRemoved"));
    });
    return;
  } else if ((b = q("[data-av]"))) {
    e.preventDefault();
    DP.av = b.dataset.av === "img" ? "img" : Number(b.dataset.av);
  } else if ((b = q("[data-addf]"))) {
    const n = PR.newf.trim();
    if (!n) {
      toast(t("backend.enter_online_id"), true);
      return;
    }
    if (n === PR.id) {
      toast(t("toast.friendSelf"), true);
      return;
    }
    if (app.friends.friends.some((f) => f.onlineId === n) || app.friends.outgoing.includes(n)) {
      toast(t("toast.friendAlready", { name: n }), true);
      return;
    }
    const fr = await attempt(() => backend.sendFriendRequest(n));
    if (!fr) return;
    app.friends = fr;
    PR.newf = "";
    toast(t("toast.friendRequestSent", { name: n }));
  } else if ((b = q("[data-acc]"))) {
    const n = b.dataset.acc ?? "";
    const fr = await attempt(() => backend.acceptFriendRequest(n));
    if (!fr) return;
    app.friends = fr;
    toast(t("toast.friendAdded", { name: n }));
  } else if ((b = q("[data-dec]"))) {
    const n = b.dataset.dec ?? "";
    confirmIt(th("dialog.declineFriend.title", { name: n }), th("dialog.declineFriend.body", { name: n }), th("dialog.declineFriend.confirm"), async () => {
      const fr = await attempt(() => backend.declineFriendRequest(n));
      if (!fr) return;
      app.friends = fr;
      toast(t("toast.friendDeclined", { name: n }));
    });
    return;
  } else if ((b = q("[data-rmf]"))) {
    const n = b.dataset.rmf ?? "";
    confirmIt(th("dialog.removeFriend.title", { name: n }), th("dialog.removeFriend.body", { name: n }), th("dialog.removeFriend.confirm"), async () => {
      const fr = await attempt(() => backend.removeFriend(n));
      if (!fr) return;
      app.friends = fr;
      toast(t("toast.friendRemoved", { name: n }));
    });
    return;
  } else if ((b = q("[data-invx]"))) {
    const n = b.dataset.invx;
    app.invites = app.invites.filter((x) => x !== n);
  } else return;
  render();
}

function onInput(e: Event): void {
  const t = e.target as HTMLInputElement;
  if (t.dataset.p) {
    if (t.dataset.p === "newf") {
      PR.newf = t.value;
      return;
    }
    DP.id = t.value;
    if (PR.in) {
      const c = t.selectionStart;
      render();
      const n = root.querySelector<HTMLInputElement>('input[data-p="id"]');
      if (n) {
        n.focus();
        n.setSelectionRange(c, c);
      }
    }
    return;
  }
  const k = t.dataset.k;
  if (!isKey(k)) return;
  if (t.type === "range") {
    setSetting(k, +t.value);
    const a = M[k]?.a;
    if (a) t.style.setProperty("--v", (((+t.value - a[0]) / (a[1] - a[0])) * 100).toFixed(1) + "%");
    const o = root.querySelector(`[data-out="${k}"]`);
    if (o) o.textContent = fmt(k);
  } else setSetting(k, t.value);
}

function onChange(e: Event): void {
  const el = e.target as HTMLInputElement;
  if (el.type === "file") {
    const f = el.files?.[0];
    el.value = "";
    if (!f) return;
    if (!f.type.startsWith("image/")) {
      toast(t("toast.chooseImageFile"), true);
      return;
    }
    void picturePng(f).then(
      (png) => {
        if (!png) return toast(t("toast.imageTooLarge"), true);
        DP.img = png;
        DP.av = "img";
        render();
        toast(t("toast.imageAdded"));
      },
      () => toast(t("toast.imageUnreadable"), true),
    );
    return;
  }
  const k = el.dataset.k;
  if (isKey(k)) setSetting(k, el.type === "range" ? +el.value : el.value);
  if (k || el.dataset.p === "id") renderAfterPress();
}

let pressed = false;
let renderDue = false;

/**
 * A field's `change` fires on the mousedown that takes the focus from it. Redrawing there replaces the button under
 * the pointer before the mouse is released, and its click never comes: the redraw waits until the press is over.
 */
function renderAfterPress(): void {
  if (pressed) renderDue = true;
  else render();
}

function onPress(): void {
  pressed = true;
}

/** The click follows the release in the same task, so the timer runs after its handler. */
function onRelease(): void {
  pressed = false;
  if (!renderDue) return;
  renderDue = false;
  setTimeout(render);
}

async function onModalClick(e: MouseEvent): Promise<void> {
  const target = e.target as HTMLElement;
  const w = target.closest<HTMLButtonElement>("[data-wz]");
  if (w) {
    if (!w.disabled) await wzAct(w.dataset.wz ?? "");
    return;
  }
  const link = target.closest<HTMLButtonElement>("[data-link]");
  if (link) {
    await attempt(() => backend.openLink(link.dataset.link as ExternalLink));
    return;
  }
  const r = target.closest<HTMLButtonElement>("[data-rec]");
  if (r) {
    if (r.dataset.rec === "done") recoverySaved();
    else
      navigator.clipboard.writeText(REC.code).then(
        () => toast(t("toast.recoveryCopied")),
        () => toast(t("toast.recoveryNotCopied"), true),
      );
    return;
  }
  const c = target.closest("[data-rc]");
  if (c) {
    c.classList.toggle("on");
    c.setAttribute("aria-checked", String(c.classList.contains("on")));
    const done = modalEl.querySelector<HTMLButtonElement>('[data-rec="done"]');
    if (done) done.disabled = !c.classList.contains("on");
    return;
  }
  if (target.classList.contains("b-modal")) {
    if (app.wz === null && !REC.code) closeModal();
    return;
  }
  const b = target.closest<HTMLButtonElement>("[data-m]");
  const current = MODAL;
  if (!b || !current) return;
  const a = current.actions[Number(b.dataset.m)];
  if (!a) return;
  if (a.fn) {
    const buttons = modalEl.querySelectorAll<HTMLButtonElement>("[data-m]");
    buttons.forEach((x) => (x.disabled = true));
    const r = await a.fn(modalEl);
    buttons.forEach((x) => (x.disabled = false));
    if (r === false) return;
    if (MODAL !== current) {
      render();
      return;
    }
  }
  closeModal();
  render();
}

/** The setup assistant's language pickers save at once; the launcher language also redraws everything in it. */
function onModalChange(e: Event): void {
  const el = e.target as HTMLSelectElement;
  const k = el.dataset.wzk;
  if (k !== "launcherLanguage" && k !== "gameLanguage") return;
  setSetting(k, el.value);
  (SAVED as unknown as Record<string, unknown>)[k] = S[k];
  void attempt(() => backend.saveSettings({ ...SAVED }));
  if (k === "launcherLanguage") setLang(S.launcherLanguage);
  render();
  wizard();
  toast(t(k === "launcherLanguage" ? "wizard.toast.languageSaved" : "toast.changesSaved"));
}

function onModalInput(e: Event): void {
  const el = e.target as HTMLInputElement;
  if (el.dataset.wzf) wzTyped(el.dataset.wzf, el.value);
}

export function bindEvents(): void {
  root.addEventListener("click", (e) => void onClick(e));
  root.addEventListener("input", onInput);
  root.addEventListener("change", onChange);
  addEventListener("pointerdown", onPress, true);
  addEventListener("pointerup", onRelease, true);
  addEventListener("pointercancel", onRelease, true);
  modalEl.addEventListener("click", (e) => void onModalClick(e));
  modalEl.addEventListener("change", onModalChange);
  modalEl.addEventListener("input", onModalInput);
  addEventListener("keydown", (e) => {
    if (e.key === "Escape" && MODAL) closeModal();
  });
}

export const initialScreen = (): ScreenId | null => {
  const h = location.hash.slice(1);
  return MENU.some((n) => n[0] === h) ? (h as ScreenId) : null;
};

