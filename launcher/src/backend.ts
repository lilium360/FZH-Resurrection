import type { MockHooks } from "./backend.mock";
import { LANGS, t, tr, type Lang, type Msg, type Params } from "./i18n";

export type { Msg } from "./i18n";

export type LauncherLanguage = Lang;
export const LAUNCHER_LANGUAGES: readonly LauncherLanguage[] = LANGS;

export const GAME_LANGUAGES = ["default", "it-IT", "en-US", "en-GB", "fr-FR", "de-DE", "es-ES", "pt-PT", "ru-RU", "pl-PL"] as const;
export type GameLanguage = (typeof GAME_LANGUAGES)[number];

export const FRAME_RATES = ["60", "90", "120"] as const;
export type FrameRate = (typeof FRAME_RATES)[number];

export const PIXEL_DENSITIES = ["default", "0.5", "0.7", "0.85", "1.2", "1.4", "1.6", "1.8", "2.0"] as const;
export type PixelDensity = (typeof PIXEL_DENSITIES)[number];

export const SHADOW_QUALITIES = ["default", "0"] as const;
export type ShadowQuality = (typeof SHADOW_QUALITIES)[number];

export const ANISOTROPIC_FILTERS = ["default", "4", "8", "16"] as const;
export type AnisotropicFiltering = (typeof ANISOTROPIC_FILTERS)[number];

export const ANTIALIASING_MODES = ["default", "none", "taa", "msaa"] as const;
export type Antialiasing = (typeof ANTIALIASING_MODES)[number];

export type Controller = "aim" | "pad";

export interface Settings {
  launcherLanguage: LauncherLanguage;
  /** "off", "default" or an id from listAudioDevices().inputs */
  microphone: string;
  /** "default" or an id from listAudioDevices().outputs */
  audioOutput: string;
  gameLanguage: GameLanguage;
  controller: Controller;
  gripRight: number;
  gripUp: number;
  gripForward: number;
  /** Degrees: muzzle up, muzzle to the right, clockwise seen from behind. */
  gripPitch: number;
  gripYaw: number;
  gripRoll: number;
  frameRate: FrameRate;
  asyncShaders: boolean;
  pixelDensity: PixelDensity;
  sharpness: number;
  fieldOfView: number;
  shadowQuality: ShadowQuality;
  anisotropicFiltering: AnisotropicFiltering;
  antialiasing: Antialiasing;
  /** Frames per second in the game window (emulator --show-fps). */
  fpsCounter: boolean;
  /** Frame timing in the emulator log (SHADPS4_FRAME_STATS=1). */
  frameStats: boolean;
  /** Mod: no "face the camera" warning when the player turns round (optional game patch). */
  modFreeTurn: boolean;
  usePrivateServer: boolean;
  privateServer: string;
}

export const DEFAULT_SETTINGS: Settings = {
  launcherLanguage: "en",
  microphone: "default",
  audioOutput: "default",
  gameLanguage: "default",
  controller: "aim",
  gripRight: 0,
  gripUp: 0.07,
  gripForward: 0.3,
  gripPitch: 0,
  gripYaw: 0,
  gripRoll: 0,
  frameRate: "90",
  asyncShaders: true,
  pixelDensity: "default",
  sharpness: 0,
  fieldOfView: 100,
  shadowQuality: "default",
  anisotropicFiltering: "default",
  antialiasing: "default",
  fpsCounter: false,
  frameStats: false,
  modFreeTurn: false,
  usePrivateServer: false,
  privateServer: "",
};

export interface AudioDevice {
  id: string;
  label: string;
}

export interface AudioDevices {
  inputs: AudioDevice[];
  outputs: AudioDevice[];
}

export type DumpKind = "base" | "update";
/** Which folder dialog: the game folder, the missing half after one was recognised, or where PKG files are installed. */
export type PickKind = "game" | DumpKind | "install";
/** A folder used in place, or a PKG file the launcher extracts first. */
export type DumpSource = "folder" | "pkg";

export interface DumpInfo {
  path: string;
  titleId: string;
  region: string;
  /** As in param.sfo, e.g. "01.00" or "01.43" */
  version: string;
  source: DumpSource;
}

/** Where the chosen PKG files are extracted. */
export interface InstallInfo {
  /** The parent folder the player chose (default under the launcher data folder). */
  location: string;
  /** The game folder inside it, e.g. <location>\\CUSA11182. */
  target: string;
  complete: boolean;
  running: boolean;
  /** Bytes of the game already extracted there. */
  size: number;
}

/** A folder extracted earlier from PKG files that the current choice does not use. */
export interface Leftover {
  target: string;
  size: number;
}

export interface InstallPlan {
  location: string;
  target: string;
  total: number;
  /** Already extracted there (complete files and resumable parts). */
  present: number;
  required: number;
  /** Free bytes on that drive; null when unknown. */
  free: number | null;
  /** Why the installation cannot start there, e.g. not enough space; null when it can. */
  error: Msg | null;
}

export interface InstallProgress {
  done: number;
  total: number;
  /** The file being extracted, relative to the game folder. */
  file: string;
}

export interface GameFiles {
  base: DumpInfo | null;
  update: DumpInfo | null;
  /** Folders linked, mod pak written, emulator patches installed. A merged folder has base.path === update.path. */
  prepared: boolean;
  /** Null when neither half is a PKG file. */
  install: InstallInfo | null;
  leftovers: Leftover[];
}

/** Result of selectGameFolder. `note` explains a rejected counterpart found next to the recognised half. */
export type FolderCheck =
  | { status: "ready"; files: GameFiles }
  | { status: "needUpdate"; base: DumpInfo; note: Msg | null }
  | { status: "needBase"; update: DumpInfo; note: Msg | null }
  | { status: "rejected"; path: string; found: Msg; error: Msg };

export type PrepareStage = "linking" | "mod" | "patches" | "done";

export interface PrepareProgress {
  stage: PrepareStage;
  percent: number;
}

export interface GameState {
  running: boolean;
  exitCode: number | null;
}

/** Which controller buttons reached the game in the last session (emulator log). */
export interface ControllerCheck {
  /** OpenXR interaction profile of the right controller, "" when unknown. */
  profile: string;
  /** Presses by gamepad button id: "cross", "circle", "l1", ... */
  presses: Record<string, number>;
}

export const CONTROLLER_BUTTONS = ["cross", "square", "circle", "triangle", "options", "l1", "r1", "l2", "r2", "l3", "r3"] as const;

export type ShaderCacheStatus = "ready" | "empty" | "warming";

export interface ShaderCacheState {
  status: ShaderCacheStatus;
  /** The round being played while warming, else 0. */
  round: number;
  /** The most rounds a warm-up plays. */
  rounds: number;
  /** The fewest rounds that can end it (two in a row with almost no new shaders). */
  minRounds: number;
  /** How long one round plays, from the input script; 0 when the script is missing. */
  roundSeconds: number;
}

export interface Session {
  onlineId: string;
  email: string;
  /** A preset index (0–4), or "image" for the player's own picture. */
  avatar: number | "image";
  /** The player's own picture as a `data:image/png;base64,` URL of a 256×256 PNG, kept even while a preset is chosen; null when there is none. */
  image: string | null;
}

export interface AccountState {
  session: Session | null;
  lastOnlineId: string;
  /** The server answered the launcher's last sign-in with "already signed in": another launcher, a game or another PC holds the account, and a launch is refused. */
  inUseElsewhere: boolean;
}

/** The account exists once this arrives: the code is shown once, even when `problem` says that storing it on this PC or signing in failed. */
export interface Registration {
  /** Five groups of five symbols; the launcher never stores it. */
  recoveryCode: string;
  /** Null when the account could not be stored on this PC. */
  session: Session | null;
  problem: Msg | null;
}

export interface ProfileUpdate {
  onlineId: string;
  avatar: number | "image";
  /** As `Session.image`; null deletes the stored picture. */
  image: string | null;
}

/** The avatar the server gives the game: 256×256 PNG, at most 256 KB. */
export const AVATAR_SIDE = 256;
export const AVATAR_MAX_BYTES = 256 * 1024;

export type Presence =
  | { state: "offline"; lastSeen: string | null }
  | { state: "online" }
  | { state: "menu" }
  | { state: "in-game"; mode: string };

export interface Friend {
  onlineId: string;
  online: boolean;
  presence: Presence;
}

export interface FriendsState {
  friends: Friend[];
  incoming: string[];
  outgoing: string[];
}

/** A friend invited the player to a match; the game answers it, the launcher only tells. */
export interface Invite {
  from: string;
}

export interface ServerStatus {
  /** network && progression */
  online: boolean;
  players: number | null;
  /** The server address; never shown in the UI. */
  region: string;
  /** shadNet (TCP 31313) reachable. */
  network: boolean;
  /** Progression backend (TCP 1338) reachable. */
  progression: boolean;
}

export interface Relay {
  /** English, as the list has it: "Europe", "North America", ... */
  region: string;
  location: string;
  operator: string;
  /** Null: the server has not checked it yet. */
  online: boolean | null;
}

/** The relays the server in use lists and what its own last check found; the launcher never asks a relay. */
export interface RelayList {
  /** ISO time of the server's last check; null before the first one. */
  checkedAt: string | null;
  relays: Relay[];
}

export interface HostingState {
  running: boolean;
  services: { network: boolean; progression: boolean; relay: boolean };
}

export interface AppInfo {
  launcherVersion: string;
  emulatorCommit: string;
  /** The server used when "private server" is off (build-time FZH_DEFAULT_SERVER). Never shown in the UI. */
  defaultServer: string;
  /** This build has an update key: false shows the version without update controls. */
  updates: boolean;
  /** Hosting the services from the launcher works: false disables "Host on this PC". */
  hosting: boolean;
}

export interface UpdateInfo {
  available: boolean;
  current: string;
  latest: string | null;
  notes: string[];
}

export interface UpdateProgress {
  downloaded: number;
  total: number | null;
}

export interface ProblemReport {
  description: string;
  attachLogs: boolean;
  attachSystem: boolean;
}

export interface LauncherState {
  setupCompleted: boolean;
}

/** `line` is English; `key` and `params` translate it. Emulator output has no key. */
export interface LogLine {
  time: string;
  line: string;
  key?: string;
  params?: Params;
}

/** The source repository, its licence, the bundled licence texts, the page where a problem report is filed, how to host a relay. */
export type ExternalLink = "source" | "licence" | "licences" | "issues" | "relays";

export type BackendErrorCode =
  | "invalid_input"
  | "not_found"
  | "already_exists"
  | "already_friend"
  | "blocked"
  | "wrong_password"
  | "already_signed_in"
  | "unauthorized"
  | "not_signed_in"
  | "server_unreachable"
  | "game_running"
  | "game_not_ready"
  | "busy"
  | "unsupported"
  | "io"
  | "internal";

/** `key` and `params` are the text for the player; `message` is the English fallback. */
export class BackendError extends Error {
  readonly code: BackendErrorCode;
  readonly key: string;
  readonly params: Params;
  constructor(code: BackendErrorCode, key: string, message: string, params: Params = {}) {
    super(message);
    this.name = "BackendError";
    this.code = code;
    this.key = key;
    this.params = params;
  }
  get msg(): Msg {
    return { key: this.key, params: this.params, message: this.message };
  }
}

export interface BackendEvents {
  log: LogLine;
  "game-state": GameState;
  "prepare-progress": PrepareProgress;
  "install-progress": InstallProgress;
  "cache-progress": ShaderCacheState;
  friends: FriendsState;
  /** The server refused the stored account and the launcher signed it out (`session` null), or `inUseElsewhere` changed. */
  account: AccountState;
  invite: Invite;
  hosting: HostingState;
  "server-status": ServerStatus;
  "update-progress": UpdateProgress;
}

export type Unsubscribe = () => void;

export interface Backend {
  readonly kind: "tauri" | "mock";

  getAppInfo(): Promise<AppInfo>;
  getLauncherState(): Promise<LauncherState>;
  setSetupCompleted(done: boolean): Promise<void>;
  checkForUpdate(): Promise<UpdateInfo>;
  /** Downloads the update found by the last check; emits `update-progress`. */
  downloadUpdate(): Promise<void>;
  /** Starts the installer of the downloaded update: the launcher closes and opens again. */
  installUpdate(): Promise<void>;
  openLink(target: ExternalLink): Promise<void>;
  /** Asks where to save the report (dialog title in the launcher language); the full path, or null when the player cancels. */
  saveProblemReport(report: ProblemReport, title: string): Promise<string | null>;

  loadSettings(): Promise<Settings>;
  /** A change of server also changes the account: the one this PC remembers for the new server, or none (`getAccount` tells). */
  saveSettings(settings: Settings): Promise<Settings>;
  listAudioDevices(): Promise<AudioDevices>;

  getGameFiles(): Promise<GameFiles>;
  /** `title`: the folder dialog's title in the launcher language. */
  pickFolder(kind: PickKind, title: string): Promise<string | null>;
  /** `expect` null: the game folder; "base"/"update": the half the player was asked for. */
  selectGameFolder(path: string, expect: DumpKind | null): Promise<FolderCheck>;
  resetGameFiles(): Promise<GameFiles>;
  prepareGame(): Promise<GameFiles>;
  /** What installing the chosen PKG files needs at the current install location. */
  getInstallPlan(): Promise<InstallPlan>;
  setInstallLocation(path: string): Promise<InstallPlan>;
  /** Extracts the PKG files; resolves when done or cancelled (install.complete false then). */
  installGame(): Promise<GameFiles>;
  cancelInstall(): Promise<void>;
  /** Deletes a folder the launcher extracted (only the files it wrote). */
  deleteInstall(target: string): Promise<GameFiles>;

  getGameState(): Promise<GameState>;
  /** Resolves with a warning for the player when the server did not confirm the account for this game (the game starts anyway). */
  launchGame(): Promise<Msg | null>;
  stopGame(): Promise<void>;

  getControllerCheck(): Promise<ControllerCheck>;

  getShaderCache(): Promise<ShaderCacheState>;
  startShaderWarmup(): Promise<ShaderCacheState>;
  cancelShaderWarmup(): Promise<ShaderCacheState>;
  clearShaderCache(): Promise<ShaderCacheState>;

  getAccount(): Promise<AccountState>;
  /** Signs in to an existing account with its recovery code (any case, dashes optional). */
  signIn(onlineId: string, recoveryCode: string): Promise<Session>;
  /** Creates an account with a new recovery code and signs in. */
  register(onlineId: string): Promise<Registration>;
  /** Also forgets the account of the server in use on this PC. */
  signOut(): Promise<void>;
  updateProfile(profile: ProfileUpdate): Promise<Session>;
  /** Sends the avatar the player sees (a 256×256 PNG data URL) to the server, where the game reads it; the outcome is also logged. */
  uploadAvatar(image: string): Promise<void>;
  setRecoveryEmail(email: string): Promise<Session>;

  getFriends(): Promise<FriendsState>;
  sendFriendRequest(onlineId: string): Promise<FriendsState>;
  acceptFriendRequest(onlineId: string): Promise<FriendsState>;
  declineFriendRequest(onlineId: string): Promise<FriendsState>;
  removeFriend(onlineId: string): Promise<FriendsState>;

  getServerStatus(): Promise<ServerStatus>;
  /** Null when the server in use publishes no relay list; fails when it cannot be asked. */
  getRelays(): Promise<RelayList | null>;
  getHosting(): Promise<HostingState>;
  startHosting(): Promise<HostingState>;
  stopHosting(): Promise<HostingState>;

  on<K extends keyof BackendEvents>(event: K, handler: (payload: BackendEvents[K]) => void): Promise<Unsubscribe>;
}

export function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export async function loadBackend(hooks: MockHooks): Promise<Backend> {
  const forceMock = new URLSearchParams(location.search).get("backend") === "mock";
  if (isTauri() && !forceMock) {
    const { createTauriBackend } = await import("./backend.tauri");
    return createTauriBackend();
  }
  const { createMockBackend } = await import("./backend.mock");
  return createMockBackend(hooks);
}

/** The text for the player of a failed call. */
export function errorMessage(e: unknown): string {
  if (e instanceof BackendError) return tr(e.msg) || t("toast.somethingWentWrong");
  if (e instanceof Error && e.message) return e.message;
  return typeof e === "string" && e ? e : t("toast.somethingWentWrong");
}
