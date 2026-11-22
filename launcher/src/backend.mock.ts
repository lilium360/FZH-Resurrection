import {
  BackendError,
  DEFAULT_SETTINGS,
  type Backend,
  type BackendEvents,
  type DumpInfo,
  type FolderCheck,
  type FriendsState,
  type GameFiles,
  type HostingState,
  type InstallPlan,
  type Msg,
  type PickKind,
  type Session,
  type Settings,
  type ShaderCacheState,
} from "./backend";
import { systemLang, type Params } from "./i18n";

export interface MockFolderOption {
  path: string;
  label: string;
  wrong: boolean;
}

export interface MockHooks {
  /** Stands in for the Windows folder picker: resolves to the chosen path, or null on cancel. */
  chooseFolder(title: string, options: MockFolderOption[]): Promise<string | null>;
}

/** A folder of the simulated disk: what the backend would find in it (itself or its direct subfolders). */
interface MockFolder {
  path: string;
  label: string;
  base?: DumpInfo;
  update?: DumpInfo;
  found?: Msg;
  error?: Msg;
}

const msg = (key: string, message: string, params: Params = {}): Msg => ({ key, message, params });
const err = (code: BackendError["code"], key: string, message: string, params: Params = {}) => new BackendError(code, key, message, params);
const region = (key: string, message: string): Msg => msg(`dump.region.${key}`, message);
const release = (name: string, reg: Msg, version: string): Msg =>
  msg("dump.foundRelease", `${name} · ${reg.message} · v${version}`, { name, region: reg, version });
const oldUpdate = msg("dump.wrongUpdateVersion", "Update 1.40 found: version 1.43 is needed.", { version: "1.40" });
const busyInstalling = () => err("busy", "backend.busy_installing", "The game is being installed: wait until it ends or cancel it on the Game screen.");
const notReady = () => err("game_not_ready", "backend.not_ready_choose", "Choose the game folder first.");

const CODE_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/** As the Rust side: upper case, spaces and dashes dropped, O read as 0, I and L as 1. */
const canonicalCode = (text: string): string | null => {
  const c = text.toUpperCase().replace(/[\s-]/g, "").replace(/O/g, "0").replace(/[IL]/g, "1");
  return c.length === 25 && [...c].every((x) => CODE_ALPHABET.includes(x)) ? c : null;
};
const newCode = (): string => {
  const raw = [...crypto.getRandomValues(new Uint8Array(25))].map((b) => CODE_ALPHABET[b & 31]).join("");
  return raw.match(/.{5}/g)!.join("-");
};
const ONLINE_ID = /^[A-Za-z0-9_-]{3,16}$/;
const badOnlineId = () => err("invalid_input", "backend.online_id_format", "Online ID: 3 to 16 letters, digits, - and _.");
const unreachableAccounts = (server: Msg) =>
  err("server_unreachable", "backend.account_unreachable", `Cannot reach the account service of the ${server.message}.`, { server });

const info = (path: string, version: string, source: "folder" | "pkg" = "folder"): DumpInfo => ({ path, titleId: "CUSA11182", region: "Europe", version, source });
const MERGED = "D:\\Games\\Firewall Zero Hour\\CUSA11182";
const DUMP = "D:\\Games\\FZH dump";
const BASE = "D:\\Games\\FZH base\\CUSA11182";
const UPDATE = "D:\\Games\\FZH update\\CUSA11182-patch";
const PKGS = "D:\\Games\\FZH PKG";
const PKG_BASE = "D:\\Games\\FZH PKG base";
const PKG_UPDATE = "D:\\Games\\FZH PKG update";
const GiB = 2 ** 30;
const GAME_SIZE = 43050220347;
const LOCATIONS: { path: string; free: number }[] = [
  { path: "D:\\Games", free: 96.5 * GiB },
  { path: "E:\\Games", free: 410 * GiB },
  { path: "F:\\", free: 12 * GiB },
];

const DISK: MockFolder[] = [
  { path: MERGED, label: "Merged folder · CUSA11182 · base + update 1.43", base: info(MERGED, "01.00"), update: info(MERGED, "01.43") },
  { path: DUMP, label: "Folder with both dumps inside", base: info(`${DUMP}\\CUSA11182`, "01.00"), update: info(`${DUMP}\\CUSA11182-patch`, "01.43") },
  { path: BASE, label: "Base game only · CUSA11182 v01.00", base: info(BASE, "01.00") },
  { path: UPDATE, label: "Update only · CUSA11182-patch v01.43", update: info(UPDATE, "01.43") },
  { path: PKGS, label: "PKG files · base game + update 1.43", base: info(`${PKGS}\\game.pkg`, "01.00", "pkg"), update: info(`${PKGS}\\update.pkg`, "01.43", "pkg") },
  { path: PKG_BASE, label: "PKG file · base game only", base: info(`${PKG_BASE}\\CUSA11182-base.pkg`, "01.00", "pkg") },
  { path: PKG_UPDATE, label: "PKG file · update 1.43 only", update: info(`${PKG_UPDATE}\\CUSA11182-1.43.pkg`, "01.43", "pkg") },
  {
    path: "D:\\Games\\FZH PKG old",
    label: "PKG file · update 1.40",
    found: release("update-140.pkg", region("europe", "Europe"), "01.40"),
    error: oldUpdate,
  },
  {
    path: "D:\\Games\\Firewall Zero Hour US\\CUSA09831",
    label: "CUSA09831 · USA · v01.00",
    found: release("CUSA09831", region("usa", "USA"), "01.00"),
    error: msg("dump.usaRelease", "CUSA09831 is the USA release: not supported yet. Only the European release CUSA11182 has been tested."),
  },
  { path: "D:\\Games\\Old\\CUSA11182-patch", label: "CUSA11182-patch · v01.40", found: release("CUSA11182-patch", region("europe", "Europe"), "01.40"), error: oldUpdate },
];

const OFFERED: Record<PickKind, string[]> = {
  game: DISK.map((f) => f.path),
  update: [UPDATE, PKG_UPDATE, BASE, "D:\\Games\\Old\\CUSA11182-patch", "D:\\Games\\FZH PKG old"],
  base: [BASE, PKG_BASE, UPDATE, "D:\\Games\\Firewall Zero Hour US\\CUSA09831"],
  install: [],
};

const now = () => new Date().toTimeString().slice(0, 8);
const wait = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

export function createMockBackend(hooks: MockHooks): Backend {
  const listeners = new Map<keyof BackendEvents, Set<(p: never) => void>>();
  const emit = <K extends keyof BackendEvents>(event: K, payload: BackendEvents[K]) => {
    for (const h of listeners.get(event) ?? []) (h as (p: BackendEvents[K]) => void)(payload);
  };
  const log = (key: string, line: string, params: Params = {}) => emit("log", { time: now(), line, key, params });

  let settings: Settings = { ...DEFAULT_SETTINGS, launcherLanguage: systemLang(), privateServer: "203.0.113.60" };
  let setupCompleted = false;
  const files: GameFiles = { base: null, update: null, prepared: false, install: null, leftovers: [] };
  let chosenLocation: string | null = null;
  const pkgDir = () => [files.base, files.update].find((d) => d?.source === "pkg")?.path.replace(/\\[^\\]*$/, "") ?? "D:\\Games";
  const loc = () => chosenLocation ?? `${pkgDir()}\\FZH Resurrection`;
  /** Extracted folders: bytes there and whether complete. */
  const installs = new Map<string, { done: number; complete: boolean }>();
  let installing = false;
  let cancelInstall = false;
  const pkgChosen = () => files.base?.source === "pkg" || files.update?.source === "pkg";
  const target = () => (pkgChosen() ? `${loc()}\\${files.base?.source === "pkg" ? "CUSA11182" : "CUSA11182-patch"}` : null);
  const snapshot = (): GameFiles => {
    const t = target();
    const rec = t ? installs.get(t) : undefined;
    files.install = t ? { location: loc(), target: t, complete: !!rec?.complete, running: installing, size: rec?.done ?? 0 } : null;
    files.leftovers = [...installs.entries()].filter(([k]) => k !== t).map(([k, v]) => ({ target: k, size: v.done }));
    return clone(files);
  };
  const plan = (): InstallPlan => {
    const t = target();
    if (!t) throw err("game_not_ready", "backend.install_no_pkg", "No PKG file is chosen: nothing to install.");
    const present = installs.get(t)?.done ?? 0;
    const total = files.base?.source === "pkg" ? GAME_SIZE : 1.1 * GiB;
    const free = LOCATIONS.find((l) => t.startsWith(l.path))?.free ?? 200 * GiB;
    const required = total - present;
    const error =
      free < required
        ? msg(
            "backend.install_no_space",
            `Not enough free space on ${t.slice(0, 2)}: ${(required / GiB).toFixed(1)} GB needed, ${(free / GiB).toFixed(1)} GB free.`,
            { drive: t.slice(0, 2), needed: { bytes: required }, free: { bytes: free } },
          )
        : null;
    return { location: loc(), target: t, total, present, required, free, error };
  };
  let running = false;
  let gameTimer: number | undefined;
  let session: Session | null = null;
  let profile: Session = { onlineId: "Player1", email: "", avatar: 0, image: null };
  let inviteTimer: number | undefined;
  /** The server in use, as the back end tells servers apart: "" for the community server, else the address without its relay part. */
  const serverOf = (s: Settings) => (s.usePrivateServer ? s.privateServer.replace(/\/.*$/, "").toLowerCase() : "");
  const serverName = (): Msg =>
    settings.usePrivateServer
      ? msg("server.private", `private server ${settings.privateServer}`, { address: settings.privateServer })
      : msg("server.official", "community server");
  type Registered = Map<string, { onlineId: string; code: string }>;
  /** Per server: lower-case Online ID -> registered casing and canonical recovery code. "Player1" has the contract's test code on every server. */
  const registered = new Map<string, Registered>();
  const accounts = (): Registered => {
    const key = serverOf(settings);
    let list = registered.get(key);
    if (!list) registered.set(key, (list = new Map([["player1", { onlineId: "Player1", code: canonicalCode("ABCDE-FGHJK-MNPQR-STVWX-YZ012")! }]])));
    return list;
  };
  /** The account each server was left with. */
  const remembered = new Map<string, Session>();
  let hosting: HostingState = { running: false, services: { network: false, progression: false, relay: false } };
  const cache: ShaderCacheState = { status: "ready", round: 0, rounds: 8, minRounds: 2, roundSeconds: 345 };
  const update = { current: "0.1.0", latest: "0.2.0", downloaded: false, installed: false };
  const twoDaysAgo = new Date(Date.now() - 2 * 86400000).toISOString();
  const friends: FriendsState = {
    friends: [
      { onlineId: "Player2", online: true, presence: { state: "in-game", mode: "TRAINING" } },
      { onlineId: "Nightowl", online: true, presence: { state: "online" } },
      { onlineId: "Ghost_42", online: false, presence: { state: "offline", lastSeen: twoDaysAgo } },
    ],
    incoming: ["Kestrel"],
    outgoing: [],
  };

  const requireSession = (): Session => {
    if (!session) throw err("not_signed_in", "backend.not_signed_in_short", "Sign in first");
    return session;
  };

  const scheduleInvite = () => {
    clearTimeout(inviteTimer);
    inviteTimer = window.setTimeout(() => {
      if (session && !running) emit("invite", { from: "Player2" });
    }, 4000);
  };

  /** The server answers that the account taken up again is gone. */
  const refuse = () => {
    if (!session) return;
    log("log.event.accountRefused", `Network: the server no longer accepts the account ${session.onlineId}: signed out`, { id: session.onlineId });
    session = null;
    emit("account", { session: null, lastOnlineId: profile.onlineId });
  };

  const startGame = () => {
    running = true;
    emit("game-state", { running: true, exitCode: null });
    const server = serverName();
    const density = settings.pixelDensity === "default" ? "1.0" : settings.pixelDensity;
    const lines: [string, string, Params][] = [
      ["log.event.playingOn", `Launcher: playing on the ${server.message}`, { server }],
      ["log.event.gameStarted", "Launcher: game started (process 4242)", { pid: 4242 }],
      ["", `[Emulator] started · ${settings.frameRate} fps · density ${density}${settings.fpsCounter ? " · FPS counter on" : ""}`, {}],
      ["", "[OpenXR] session opened", {}],
      ["", `[Np] ${session?.onlineId ?? profile.onlineId} signed in`, {}],
    ];
    let i = 0;
    const tick = () => {
      const line = lines[i++];
      if (!running || line === undefined) return;
      log(...line);
      gameTimer = window.setTimeout(tick, 450);
    };
    gameTimer = window.setTimeout(tick, 200);
  };

  const stop = () => {
    clearTimeout(gameTimer);
    if (!running) return;
    running = false;
    emit("game-state", { running: false, exitCode: 0 });
  };

  return {
    kind: "mock",

    async getAppInfo() {
      return { launcherVersion: update.installed ? update.latest : update.current, emulatorCommit: "4fef3a822b37f6a11cab87e33962314fbb0c9df8", defaultServer: "official.example", updates: true, hosting: true };
    },
    async getLauncherState() {
      return { setupCompleted };
    },
    async setSetupCompleted(done) {
      setupCompleted = done;
    },
    async checkForUpdate() {
      return update.installed
        ? { available: false, current: update.latest, latest: null, notes: [] }
        : {
            available: true,
            current: update.current,
            latest: update.latest,
            notes: [
              "One step for the game folder: the launcher recognises the dump by itself.",
              "Fixed the pixel density values.",
            ],
          };
    },
    async downloadUpdate() {
      if (running) throw err("game_running", "backend.update_game_running", "Close the game before updating the launcher.");
      const total = 12_000_000;
      for (let done = 0; done <= total; done += total / 8) {
        emit("update-progress", { downloaded: done, total });
        await wait(250);
      }
      update.downloaded = true;
    },
    async installUpdate() {
      if (running) throw err("game_running", "backend.update_game_running", "Close the game before updating the launcher.");
      if (!update.downloaded) throw err("invalid_input", "backend.update_not_ready", "The update is not downloaded yet.");
      update.installed = true;
      location.reload();
    },
    async openLink() {},
    async saveProblemReport() {
      return `C:\\Users\\Player\\Desktop\\fzh-report-${new Date().toISOString().slice(0, 10)}.txt`;
    },

    async loadSettings() {
      return { ...settings };
    },
    async saveSettings(next) {
      if (next.usePrivateServer && !/^[A-Za-z0-9._-]+((:\d{1,5}){5})?(\/[A-Za-z0-9._-]+(:\d{1,5})?)?$/.test(next.privateServer.trim())) {
        throw err("invalid_input", "backend.invalid_server_address", "The server address can only hold letters, digits, dots, hyphens and underscores");
      }
      const saved = { ...next, privateServer: next.privateServer.trim() };
      const [from, to] = [serverOf(settings), serverOf(saved)];
      if (from !== to && running) throw err("game_running", "backend.account_game_running", "Close the game first: it uses the account while it runs.");
      settings = saved;
      if (from !== to) {
        clearTimeout(inviteTimer);
        if (session) remembered.set(from, session);
        else remembered.delete(from);
        session = remembered.get(to) ?? null;
        if (session) {
          profile = { ...session };
          log("log.event.accountRemembered", `Network: back on the ${serverName().message} with the account ${session.onlineId}`, { id: session.onlineId, server: serverName() });
          if (session.onlineId.toLowerCase().startsWith("refused")) window.setTimeout(refuse, 1500);
          else scheduleInvite();
        }
      }
      return { ...settings };
    },
    async listAudioDevices() {
      return {
        inputs: [
          { id: "mic_usb", label: "Microphone (USB Audio Device)" },
          { id: "mic_realtek", label: "Microphone (Realtek Audio)" },
        ],
        outputs: [
          { id: "out_usb", label: "Headphones (USB Audio Device)" },
          { id: "out_realtek", label: "Speakers (Realtek Audio)" },
        ],
      };
    },

    async getGameFiles() {
      return snapshot();
    },
    async pickFolder(kind, title) {
      if (kind === "install")
        return hooks.chooseFolder(
          title,
          LOCATIONS.map((l) => ({ path: l.path, label: `${l.path} · ${(l.free / GiB).toFixed(0)} GB free`, wrong: l.free < GAME_SIZE })),
        );
      const wrong = (f: MockFolder) => !!f.error || (kind === "base" && !f.base) || (kind === "update" && !f.update);
      return hooks.chooseFolder(
        title,
        OFFERED[kind].map((p) => DISK.find((f) => f.path === p)!).map((f) => ({ path: f.path, label: f.label, wrong: wrong(f) })),
      );
    },
    async selectGameFolder(path, expect): Promise<FolderCheck> {
      if (installing) throw busyInstalling();
      const f = DISK.find((x) => x.path === path);
      files.prepared = false;
      if (!expect) chosenLocation = null;
      const reject = (found: Msg, error: Msg): FolderCheck => {
        if (!expect) files.base = files.update = null;
        return { status: "rejected", path, found, error };
      };
      const name = path.split("\\").pop() ?? path;
      const folder = msg("dump.noun.folder", "This folder");
      if (!f) return reject(msg("", name), msg("dump.noPkgNoDump", "No PS4 game here: this folder and its subfolders hold no PKG files and no game dump.", {}));
      if (f.error) return reject(f.found ?? msg("", name), f.error);
      if (expect === "update" && !f.update)
        return reject(
          msg("dump.foundKind", `${name} · base game`, { name, kind: msg("file.base", "base game") }),
          msg("dump.holdsBase", "This folder holds the base game, not the update.", { noun: folder }),
        );
      if (expect === "base" && !f.base)
        return reject(
          msg("dump.foundKind", `${name} · update`, { name, kind: msg("file.update", "update") }),
          msg("dump.holdsUpdate", "This folder holds the update, not the base game.", { noun: folder }),
        );
      if (f.base && f.update) [files.base, files.update] = [{ ...f.base }, { ...f.update }];
      else if (expect === "update" && f.update) files.update = { ...f.update };
      else if (expect === "base" && f.base) files.base = { ...f.base };
      else [files.base, files.update] = [f.base ? { ...f.base } : null, f.update ? { ...f.update } : null];
      if (files.base && files.update) return { status: "ready", files: snapshot() };
      return files.base ? { status: "needUpdate", base: clone(files.base), note: null } : { status: "needBase", update: clone(files.update!), note: null };
    },
    async resetGameFiles() {
      if (installing) throw busyInstalling();
      files.base = files.update = null;
      files.prepared = false;
      return snapshot();
    },
    async prepareGame() {
      if (!files.base || !files.update) throw notReady();
      if (pkgChosen() && !installs.get(target()!)?.complete) throw err("game_not_ready", "backend.not_ready_install", "Install the game from your PKG files first.");
      emit("prepare-progress", { stage: "linking", percent: 0 });
      if (files.base.path === files.update.path) log("log.event.linkedMerged", "Preparing: merged game folder linked, nothing copied");
      else log("log.event.linkedSeparate", "Preparing: base game and update linked, nothing copied");
      await wait(500);
      emit("prepare-progress", { stage: "mod", percent: 40 });
      log("log.event.modPakWritten", "Preparing: mod pak written (52596 bytes)", { size: { bytes: 52596 } });
      await wait(500);
      emit("prepare-progress", { stage: "patches", percent: 75 });
      log("log.event.patchesInstalled", "Preparing: emulator patches installed (fzh.xml)", { names: "fzh.xml" });
      await wait(300);
      emit("prepare-progress", { stage: "done", percent: 100 });
      files.prepared = true;
      log("log.event.gameReady", "Preparing: game ready");
      return snapshot();
    },
    async getInstallPlan() {
      await wait(300);
      return plan();
    },
    async setInstallLocation(path) {
      if (installing) throw busyInstalling();
      chosenLocation = path;
      files.prepared = false;
      return plan();
    },
    async installGame() {
      if (installing) throw busyInstalling();
      const p = plan();
      if (p.error) throw err("io", p.error.key ?? "", p.error.message ?? "", p.error.params);
      installing = true;
      cancelInstall = false;
      files.prepared = false;
      const rec = installs.get(p.target) ?? { done: 0, complete: false };
      installs.set(p.target, rec);
      log("log.event.installStarted", `Installing: ${(p.total / GiB).toFixed(1)} GB into ${p.target}`, {
        total: { bytes: p.total },
        target: p.target,
        required: { bytes: p.required },
        present: { bytes: p.present },
      });
      const step = p.total / 40;
      while (rec.done < p.total && !cancelInstall) {
        await wait(150);
        rec.done = Math.min(p.total, rec.done + step);
        emit("install-progress", { done: rec.done, total: p.total, file: rec.done < 0.01 * p.total ? "eboot.bin" : "firewall/content/paks/firewall-ps4.pak" });
      }
      installing = false;
      if (cancelInstall) log("log.event.installCancelled", "Installing: cancelled", { size: { bytes: rec.done } });
      else {
        rec.complete = true;
        log("log.event.installDone", "Installing: done in 6 s", { duration: { seconds: 6 } });
      }
      return snapshot();
    },
    async cancelInstall() {
      if (installing) cancelInstall = true;
    },
    async deleteInstall(target) {
      if (installing) throw busyInstalling();
      if (running) throw err("game_running", "backend.game_running_delete", "Close the game first: it may be using these files.");
      const rec = installs.get(target);
      if (!rec) throw err("not_found", "backend.not_found_install", "The launcher did not install a game there.");
      installs.delete(target);
      if (target === snapshot().install?.target) files.prepared = false;
      log("log.event.installDeleted", `Launcher: installed game deleted from ${target}`, { target, size: { bytes: rec.done } });
      return snapshot();
    },

    async getGameState() {
      return { running, exitCode: null };
    },
    async launchGame() {
      if (running) throw err("game_running", "backend.game_already_running", "The game is already running");
      if (!files.prepared) throw notReady();
      startGame();
      if (!session?.onlineId.toLowerCase().startsWith("nogrant")) return null;
      const reason = msg("backend.play_code_refused", "The server did not accept the launcher's confirmation.");
      log("log.event.playGrantFailed", `Network: the server did not confirm the account for this game: ${reason.message}`, { reason });
      return msg("backend.play_grant_failed", `The server did not confirm your account: ${reason.message} The game starts anyway, but it may not sign in.`, { reason });
    },
    async stopGame() {
      stop();
    },

    async getControllerCheck() {
      return { profile: "/interaction_profiles/oculus/touch_controller", presses: { cross: 4, circle: 1, r2: 12, l3: 2 } };
    },

    async getShaderCache() {
      return { ...cache };
    },
    async startShaderWarmup() {
      if (cache.status === "warming") throw err("busy", "backend.warmup_already", "The warm-up is already running");
      if (running) throw err("game_running", "backend.warmup_game", "Close the game first: the warm-up starts the game by itself.");
      requireSession();
      cache.status = "warming";
      cache.round = 1;
      const tick = () => {
        if (cache.status !== "warming") return;
        if (cache.round >= cache.rounds) {
          cache.status = "ready";
          cache.round = 0;
          emit("cache-progress", { ...cache });
          return;
        }
        cache.round++;
        emit("cache-progress", { ...cache });
        window.setTimeout(tick, 500);
      };
      window.setTimeout(tick, 500);
      return { ...cache };
    },
    async cancelShaderWarmup() {
      if (cache.status === "warming") {
        cache.status = cache.round > 1 ? "ready" : "empty";
        cache.round = 0;
        log("log.event.warmupCancelled", `Warm-up: cancelled after ${cache.round - 1} complete rounds`, { count: cache.round - 1 });
      }
      return { ...cache };
    },
    async clearShaderCache() {
      if (cache.status === "warming") throw err("busy", "backend.warmup_running", "The shader cache warm-up is running.");
      cache.status = "empty";
      cache.round = 0;
      return { ...cache };
    },

    async getAccount() {
      return { session: session && { ...session }, lastOnlineId: profile.onlineId };
    },
    async signIn(onlineId, recoveryCode) {
      if (!ONLINE_ID.test(onlineId.trim())) throw badOnlineId();
      const code = canonicalCode(recoveryCode);
      if (!code) throw err("invalid_input", "backend.recovery_code_invalid", "That is not a recovery code: it has 25 letters and digits in five groups.");
      const known = accounts().get(onlineId.trim().toLowerCase());
      if (known?.code !== code) throw err("wrong_password", "backend.recover_wrong", "Wrong Online ID or recovery code.");
      profile = { ...profile, onlineId: known.onlineId };
      session = { ...profile };
      scheduleInvite();
      return { ...session };
    },
    /** Names starting with taken, admin, busy, closed or offline show those refusals; nosign creates the account but the sign-in fails; refused is turned away when its server is chosen again. */
    async register(onlineId) {
      const id = onlineId.trim();
      if (!ONLINE_ID.test(id)) throw badOnlineId();
      const lower = id.toLowerCase();
      if (lower.startsWith("offline")) throw unreachableAccounts(serverName());
      if (lower.startsWith("busy")) throw err("busy", "backend.account_rate_limited", "Too many attempts from this network. Try again in 42 s.", { seconds: { seconds: 42 } });
      if (lower.startsWith("closed")) throw err("unsupported", "backend.register_closed", "This server does not create accounts.");
      if (lower.startsWith("admin")) throw err("invalid_input", "backend.register_name_reserved", "That Online ID is reserved. Choose another one.");
      if (lower.startsWith("taken") || accounts().has(lower)) throw err("already_exists", "backend.register_id_taken", "That Online ID is taken.");
      const recoveryCode = newCode();
      accounts().set(lower, { onlineId: id, code: canonicalCode(recoveryCode)! });
      log("log.event.accountCreated", `Network: account ${id} created on the ${serverName().message}`, { id, server: serverName() });
      profile = { ...profile, onlineId: id };
      session = { ...profile };
      if (lower.startsWith("nosign")) {
        const problem = msg("backend.net_official_unreachable", "Cannot reach the community server. Check your connection and try again.");
        return { recoveryCode, session: { ...session }, problem };
      }
      scheduleInvite();
      return { recoveryCode, session: { ...session }, problem: null };
    },
    async signOut() {
      clearTimeout(inviteTimer);
      session = null;
      remembered.delete(serverOf(settings));
    },
    async updateProfile(p) {
      requireSession();
      profile = { ...profile, onlineId: p.onlineId, avatar: p.avatar, image: p.image };
      session = { ...profile };
      return { ...session };
    },
    async uploadAvatar(image) {
      requireSession();
      if (!image.startsWith("data:image/png;base64,")) throw err("invalid_input", "backend.avatar_not_png", "The avatar is not a PNG picture.");
      log("log.event.avatarSent", "Network: avatar sent to the server");
    },
    async setRecoveryEmail(email) {
      requireSession();
      profile = { ...profile, email };
      session = { ...profile };
      return { ...session };
    },

    async getFriends() {
      return clone(friends);
    },
    async sendFriendRequest(onlineId) {
      requireSession();
      friends.outgoing.push(onlineId);
      return clone(friends);
    },
    async acceptFriendRequest(onlineId) {
      friends.incoming = friends.incoming.filter((r) => r !== onlineId);
      friends.friends.unshift({ onlineId, online: true, presence: { state: "online" } });
      return clone(friends);
    },
    async declineFriendRequest(onlineId) {
      friends.incoming = friends.incoming.filter((r) => r !== onlineId);
      return clone(friends);
    },
    async removeFriend(onlineId) {
      friends.friends = friends.friends.filter((f) => f.onlineId !== onlineId);
      friends.outgoing = friends.outgoing.filter((f) => f !== onlineId);
      return clone(friends);
    },

    async getServerStatus() {
      return { online: true, players: 128, region: "", network: true, progression: true };
    },
    async getHosting() {
      return clone(hosting);
    },
    async startHosting() {
      if (!settings.usePrivateServer) throw err("invalid_input", "toast.savePrivateFirst", "Save the private server setting first");
      hosting = { running: true, services: { network: true, progression: true, relay: true } };
      emit("hosting", clone(hosting));
      return clone(hosting);
    },
    async stopHosting() {
      hosting = { running: false, services: { network: false, progression: false, relay: false } };
      emit("hosting", clone(hosting));
      return clone(hosting);
    },

    async on<K extends keyof BackendEvents>(event: K, handler: (payload: BackendEvents[K]) => void) {
      const set = listeners.get(event) ?? new Set<(p: never) => void>();
      listeners.set(event, set);
      const h = handler as (p: never) => void;
      set.add(h);
      return () => {
        set.delete(h);
      };
    },
  };
}
