import "@fontsource/barlow/400.css";
import "@fontsource/barlow/500.css";
import "@fontsource/barlow/600.css";
import "@fontsource/barlow/700.css";
import "@fontsource/barlow/800.css";
import "@fontsource/barlow/900.css";
import "@fontsource/lekton/400.css";
import "@fontsource/lekton/700.css";
import "./styles.css";

import { loadBackend, type Backend } from "./backend";
import type { MockHooks } from "./backend.mock";
import { setLang, t } from "./i18n";
import { checkUpdate, loadPlan, prepare, showInvite, signedOut, syncAvatar } from "./ui/actions";
import { esc, updateProgress } from "./ui/components";
import { bindEvents, initialScreen } from "./ui/events";
import { initTips } from "./ui/tips";
import { ask, cancelAction, refresh, render, renderLog, setWizardRenderer, toast } from "./ui/overlay";
import { CACHE, CHECK, DP, F, PR, S, SAVED, UPD, app, applyFiles, applySession, log, logMsg, needsInstall, setBackend } from "./ui/state";
import { wizard } from "./ui/wizard";

/** In a plain browser the folder picker is a dialog listing the simulated disk's folders. */
const mockHooks: MockHooks = {
  chooseFolder: (title, options) =>
    new Promise((resolve) => {
      ask(
        esc(title),
        "",
        [
          ...options.map((o) => ({ label: esc(o.label), cls: o.wrong ? "" : "fill", fn: () => resolve(o.path) })),
          { ...cancelAction(), fn: () => resolve(null) },
        ],
        "",
        true,
      );
    }),
};

async function subscribe(backend: Backend): Promise<void> {
  await Promise.all([
    backend.on("log", (l) => {
      log({ key: l.key, params: l.params, message: l.line }, l.time);
      renderLog();
    }),
    backend.on("game-state", (s) => {
      if (s.running === app.run) return;
      app.run = s.running;
      if (!s.running) void backend.getControllerCheck().then((c) => (Object.assign(CHECK, c), render()), () => {});
      if (!s.running && !app.stopping) log(s.exitCode ? logMsg("log.event.gameExitedCode", { code: s.exitCode }) : logMsg("log.event.gameClosed"));
      render();
    }),
    backend.on("install-progress", (p) => {
      F.prog = p;
      if (!updateProgress()) refresh();
    }),
    backend.on("update-progress", (p) => {
      UPD.prog = p;
      render();
    }),
    backend.on("cache-progress", (c) => {
      const was = CACHE.status;
      Object.assign(CACHE, c);
      render();
      if (was === "warming" && c.status !== "warming") toast(t("toast.warmupEnded"));
    }),
    backend.on("friends", (f) => {
      const known = new Set(app.friends.incoming);
      const fresh = f.incoming.filter((id) => !known.has(id));
      app.friends = f;
      if (fresh.length) toast(t("toast.friendRequestReceived", { name: fresh.join(", ") }));
      render();
    }),
    backend.on("account", (a) => {
      if (a.session || !PR.in) return;
      signedOut();
      toast(t("toast.accountRefused", { id: PR.id }), true);
      refresh();
    }),
    backend.on("invite", showInvite),
    backend.on("hosting", (h) => {
      if (h.running === app.host) return;
      app.host = h.running;
      render();
    }),
    backend.on("server-status", (s) => {
      app.pub = s;
      render();
    }),
  ]);
}

async function boot(): Promise<void> {
  const deepLink = location.hash.length > 1;
  const h = initialScreen();
  const backend = await loadBackend(mockHooks);
  setBackend(backend);
  setWizardRenderer(wizard);
  bindEvents();
  await subscribe(backend);

  const [settings, devices, launcher, files, account, cache, pub, hosting, info, game] = await Promise.all([
    backend.loadSettings(),
    backend.listAudioDevices(),
    backend.getLauncherState(),
    backend.getGameFiles(),
    backend.getAccount(),
    backend.getShaderCache(),
    backend.getServerStatus().catch(() => ({ online: false, players: null, region: "", network: false, progression: false })),
    backend.getHosting(),
    backend.getAppInfo(),
    backend.getGameState(),
  ]);
  Object.assign(S, settings);
  Object.assign(SAVED, settings);
  setLang(settings.launcherLanguage);
  app.devices = devices;
  applyFiles(files);
  Object.assign(CACHE, cache);
  void backend.getControllerCheck().then((c) => Object.assign(CHECK, c), () => {});
  app.pub = pub;
  app.host = hosting.running;
  app.info = info;
  app.run = game.running;
  UPD.cur = info.launcherVersion;
  PR.id = account.lastOnlineId;
  DP.id = account.lastOnlineId;
  if (account.session) {
    applySession(account.session);
    syncAvatar();
    app.friends = await backend.getFriends().catch(() => app.friends);
  }

  if (h) app.scr = h;
  if (deepLink || launcher.setupCompleted) app.wz = null;
  render();
  wizard();
  if (F.base && F.update && needsInstall()) void loadPlan();
  else if (F.base && F.update && F.prep === 0 && !app.run && CACHE.status !== "warming") void prepare();

  void checkUpdate(true);
}

initTips();
void boot();
