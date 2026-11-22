import { BackendError, errorMessage, type Invite, type Msg, type PickKind, type Registration, type Session } from "../backend";
import { t, th, tr } from "../i18n";
import { avatarPng } from "./avatar";
import { gb, recognised } from "./components";
import { fileName, type ScreenId } from "./meta";
import { ask, cancelAction, closeModal, confirmIt, refresh, render, toast } from "./overlay";
import { DP, F, PR, REC, S, SAVED, UPD, app, applyFiles, applySession, backend, dirty, dirtyP, installing, log, logMsg, needsInstall } from "./state";

export async function attempt<T>(fn: () => Promise<T>): Promise<T | undefined> {
  try {
    return await fn();
  } catch (e) {
    toast(errorMessage(e), true);
    return undefined;
  }
}

/** A failure as a message, so it is translated again when the language changes. */
const failure = (e: unknown): Msg => (e instanceof BackendError ? e.msg : { message: errorMessage(e) });

export async function launch(): Promise<void> {
  app.run = true;
  app.invites = [];
  log(logMsg("log.event.launching"));
  toast(t("toast.launchingGame"));
  render();
  try {
    const warning = await backend.launchGame();
    if (warning) toast(tr(warning), true);
  } catch (e) {
    app.run = false;
    log(logMsg("log.event.launchFailed", { reason: failure(e) }));
    toast(errorMessage(e), true);
    render();
  }
}

export async function stopGame(line: Msg = logMsg("log.event.gameClosed")): Promise<void> {
  app.stopping = true;
  try {
    await backend.stopGame();
    app.run = false;
    log(line);
  } finally {
    app.stopping = false;
  }
}

export function go(to: ScreenId): void {
  if (to === app.scr) return;
  if (!dirty()) {
    app.scr = to;
    render();
    return;
  }
  ask(th("dialog.unsavedChanges.title"), th("dialog.unsavedChanges.body"), [
    cancelAction(),
    {
      label: th("dialog.unsavedChanges.discard"),
      cls: "danger",
      fn: () => {
        discard();
        app.scr = to;
        toast(t("toast.changesDiscarded"));
      },
    },
    {
      label: th("dialog.unsavedChanges.save"),
      cls: "fill",
      fn: () => {
        void save();
        app.scr = to;
      },
    },
  ]);
}

/** The game answers invitations: the launcher keeps a notice until the game starts or the player dismisses it. */
export function showInvite(inv: Invite): void {
  if (!PR.in || app.run) return;
  app.invites = [inv.from, ...app.invites.filter((n) => n !== inv.from)];
  toast(t("screen.friends.invitedYou", { name: inv.from }));
  render();
}

/** Links the folders, writes the mod pak and installs the emulator patches; runs by itself after a folder is recognised. */
export async function prepare(): Promise<void> {
  if (F.prep === 1 || !F.base || !F.update || needsInstall() || installing()) return;
  F.prep = 1;
  F.prepErr = null;
  refresh();
  try {
    const files = await backend.prepareGame();
    applyFiles(files);
    if (files.prepared) toast(t("toast.gameReady"));
  } catch (e) {
    F.prep = 0;
    F.prepErr = failure(e);
    toast(errorMessage(e), true);
  }
  refresh();
}

/** The folder dialog's title in the launcher language. */
const pickTitle = (k: PickKind): string => t(`folderPicker.${k}`);

export async function chooseFolder(k: Exclude<PickKind, "install">): Promise<void> {
  const path = await attempt(() => backend.pickFolder(k, pickTitle(k)));
  if (!path) return;
  const r = await attempt(() => backend.selectGameFolder(path, k === "game" ? null : k));
  if (!r) return;
  F.prep = 0;
  F.prepErr = null;
  if (r.status === "rejected") {
    if (k === "game") {
      F.base = F.update = null;
      F.inst = null;
    }
    F.err = r.error;
    F.found = r.found;
    log(logMsg("log.event.folderRejected", { path: r.path, reason: r.error }));
    toast(k === "game" ? t("toast.folderNotUsable") : t("toast.wrongHalf", { name: fileName(k).toLowerCase() }), true);
    refresh();
    return;
  }
  F.err = F.found = null;
  F.instErr = null;
  if (r.status === "ready") {
    applyFiles(r.files);
    F.note = null;
    log(logMsg("log.event.gameFolder", { what: recognised() }));
    if (needsInstall()) {
      toast(t("toast.pkgRecognised"));
      await loadPlan();
      return;
    }
    toast(t("toast.folderRecognised"));
    await prepare();
    return;
  }
  if (r.status === "needUpdate") {
    F.base = r.base;
    F.update = null;
  } else {
    F.update = r.update;
    F.base = null;
  }
  F.inst = null;
  F.note = r.note;
  log(logMsg("log.event.gameFolder", { what: recognised() }));
  toast(t(r.status === "needUpdate" ? "toast.baseFound" : "toast.updateFound"));
  refresh();
}

/** What installing needs at the current location (space, what is already there). */
export async function loadPlan(): Promise<void> {
  F.plan = null;
  F.planErr = null;
  refresh();
  try {
    F.plan = await backend.getInstallPlan();
  } catch (e) {
    F.planErr = failure(e);
  }
  refresh();
}

async function reloadFiles(): Promise<void> {
  const files = await attempt(() => backend.getGameFiles());
  if (files) applyFiles(files);
}

export async function changeLocation(): Promise<void> {
  const path = await attempt(() => backend.pickFolder("install", pickTitle("install")));
  if (!path) return;
  try {
    F.plan = await backend.setInstallLocation(path);
    F.planErr = null;
    F.instErr = null;
  } catch (e) {
    toast(errorMessage(e), true);
    return;
  }
  await reloadFiles();
  log(logMsg("log.event.installLocation", { path: F.plan.target }));
  refresh();
  if (!needsInstall()) {
    toast(t("toast.alreadyInstalled"));
    await prepare();
  }
}

export async function install(): Promise<void> {
  if (installing() || !F.plan || F.plan.error) return;
  F.instErr = null;
  F.prog = { done: F.plan.present, total: F.plan.total, file: "" };
  F.progStart = { t: Date.now(), done: F.plan.present };
  if (F.inst) F.inst.running = true;
  refresh();
  try {
    applyFiles(await backend.installGame());
    if (F.inst) F.inst.running = false;
    if (F.inst?.complete) {
      toast(t("toast.gameInstalled"));
      F.prog = null;
      await prepare();
      return;
    }
    toast(t("toast.installCancelled"));
    await loadPlan();
  } catch (e) {
    F.instErr = failure(e);
    toast(errorMessage(e), true);
    await reloadFiles();
    await loadPlan();
  } finally {
    if (F.inst) F.inst.running = false;
    F.progStart = null;
    refresh();
  }
}

export async function cancelInstall(): Promise<void> {
  await attempt(() => backend.cancelInstall());
}

export function deleteInstall(target: string, size: number): void {
  confirmIt(
    th("dialog.deleteInstall.title"),
    th("dialog.deleteInstall.body", { size: gb(size), target }),
    th("dialog.deleteInstall.confirm"),
    async () => {
      const files = await attempt(() => backend.deleteInstall(target));
      if (!files) return;
      applyFiles(files);
      toast(t("toast.deleted", { size: gb(size) }));
      if (needsInstall()) await loadPlan();
    },
  );
}

export function discard(): void {
  Object.assign(S, SAVED);
  Object.assign(DP, { id: PR.id, av: PR.av, img: PR.img });
}

export async function signOut(): Promise<void> {
  if (app.run) await attempt(() => stopGame(logMsg("log.event.gameStoppedSignOut")));
  await attempt(() => backend.signOut());
  PR.in = false;
  app.invites = [];
  app.friends = { friends: [], incoming: [], outgoing: [] };
  Object.assign(DP, { id: PR.id, av: PR.av, img: PR.img });
}

/** After a sign-in: the server gets the avatar the launcher shows. Never in the way of the sign-in; the outcome goes to the log. */
export function syncAvatar(): void {
  void avatarPng(PR)
    .then((png) => backend.uploadAvatar(png))
    .catch((e) => {
      if (!(e instanceof BackendError)) log(logMsg("log.event.avatarNotSent", { reason: failure(e) }));
    });
}

/** After a sign-in or a registration: the session, the avatar on the server, the friends list. */
export async function signedIn(s: Session): Promise<void> {
  applySession(s);
  syncAvatar();
  await loadFriends();
}

/** Registers the Online ID; the recovery code step opens at once and stays until the player confirms. */
export async function register(onlineId: string): Promise<boolean> {
  const r: Registration | undefined = await attempt(() => backend.register(onlineId));
  if (!r) return false;
  REC.code = r.recoveryCode;
  REC.problem = r.problem;
  if (r.session) await signedIn(r.session);
  refresh();
  return true;
}

/** The player confirmed they saved the code: it is forgotten. */
export function recoverySaved(): void {
  const problem = REC.problem;
  REC.code = "";
  REC.problem = null;
  closeModal();
  render();
  if (problem) toast(tr(problem), true);
  else toast(t("toast.accountCreated", { id: PR.id }));
}

export async function loadFriends(): Promise<void> {
  const fr = await attempt(() => backend.getFriends());
  if (fr) app.friends = fr;
}

export async function save(): Promise<void> {
  if (PR.in && !DP.id.trim()) {
    toast(t("toast.onlineIdEmpty"), true);
    return;
  }
  if (S.usePrivateServer && !S.privateServer.trim()) {
    toast(t("toast.emptyServer"), true);
    return;
  }
  const commit = async () => {
    const profileChanged = dirtyP();
    try {
      const saved = await backend.saveSettings({ ...S });
      Object.assign(SAVED, saved);
      Object.assign(S, saved);
      if (PR.in && profileChanged) {
        const shown = DP.av !== PR.av || (DP.av === "img" && DP.img !== PR.img);
        const s = await backend.updateProfile({ onlineId: DP.id.trim(), avatar: DP.av === "img" ? "image" : DP.av, image: DP.img });
        applySession(s);
        if (shown) await backend.uploadAvatar(await avatarPng(PR));
      }
      toast(t("toast.changesSaved"));
    } catch (e) {
      toast(errorMessage(e), true);
    }
    render();
  };
  if (PR.in && DP.id.trim() !== PR.id)
    confirmIt(
      th("dialog.changeOnlineId.title"),
      th("dialog.changeOnlineId.body", { newId: DP.id.trim(), oldId: PR.id }),
      th("dialog.changeOnlineId.confirm"),
      () => commit(),
      false,
    );
  else if (S.usePrivateServer !== SAVED.usePrivateServer && PR.in)
    confirmIt(
      th("dialog.changeServer.title"),
      S.usePrivateServer ? th("dialog.changeServer.bodyPrivate", { address: S.privateServer }) : th("dialog.changeServer.bodyOfficial"),
      th("dialog.changeServer.confirm"),
      async () => {
        await signOut();
        if (app.host) {
          const h = await attempt(() => backend.stopHosting());
          if (h) app.host = h.running;
        }
        await commit();
      },
      false,
    );
  else await commit();
}

/** `quiet`: the check at start, which shows nothing when it fails (the back end logs why). */
export async function checkUpdate(quiet: boolean): Promise<void> {
  if (UPD.phase === "checking" || UPD.phase === "downloading" || UPD.phase === "ready") return;
  UPD.phase = "checking";
  render();
  try {
    const u = await backend.checkForUpdate();
    UPD.cur = u.current;
    UPD.next = u.latest ?? "";
    UPD.notes = u.notes;
    UPD.phase = u.available ? "available" : "latest";
    if (!quiet && !u.available) toast(t("toast.latestVersion"));
  } catch (e) {
    UPD.phase = quiet ? "idle" : "failed";
    UPD.err = failure(e);
  }
  render();
}

/** A refusal (game running, warm-up, installation) leaves the update as it was. */
const refused = (e: unknown): boolean => e instanceof BackendError && (e.code === "game_running" || e.code === "busy");

export async function downloadUpdate(): Promise<void> {
  if (app.run) {
    toast(t("toast.closeBeforeUpdate"), true);
    return;
  }
  UPD.phase = "downloading";
  UPD.prog = null;
  render();
  toast(t("toast.downloading", { version: UPD.next }));
  try {
    await backend.downloadUpdate();
    UPD.phase = "ready";
    toast(t("toast.updateReady", { version: UPD.next }));
  } catch (e) {
    UPD.phase = refused(e) ? "available" : "failed";
    UPD.err = failure(e);
    toast(errorMessage(e), true);
  }
  render();
}

export function restartToUpdate(): void {
  if (app.run) {
    toast(t("toast.closeBeforeUpdate"), true);
    return;
  }
  ask(th("dialog.restartUpdate.title"), th("dialog.restartUpdate.body", { version: UPD.next }), [
    { label: th("dialog.restartUpdate.later") },
    {
      label: th("dialog.restartUpdate.confirm"),
      cls: "fill",
      fn: async () => {
        try {
          await backend.installUpdate();
        } catch (e) {
          if (!refused(e)) {
            UPD.phase = "failed";
            UPD.err = failure(e);
          }
          toast(errorMessage(e), true);
          render();
        }
      },
    },
  ]);
}
