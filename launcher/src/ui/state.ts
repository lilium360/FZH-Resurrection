import {
  DEFAULT_SETTINGS,
  type AppInfo,
  type ControllerCheck,
  type AudioDevices,
  type Backend,
  type DumpInfo,
  type DumpKind,
  type FriendsState,
  type GameFiles,
  type InstallInfo,
  type InstallPlan,
  type InstallProgress,
  type Leftover,
  type Msg,
  type ServerStatus,
  type Session,
  type Settings,
  type ShaderCacheState,
  type UpdateProgress,
} from "../backend";
import { t } from "../i18n";
import type { ArtView } from "./art";
import type { ScreenId } from "./meta";

export let backend: Backend;
export function setBackend(b: Backend): void {
  backend = b;
}

export const APP = t("app.name");

export const S: Settings = { ...DEFAULT_SETTINGS };
export const SAVED: Settings = { ...DEFAULT_SETTINGS };

export type AvatarChoice = number | "img";

export const PR = {
  in: false,
  id: "",
  av: 0 as AvatarChoice,
  img: null as string | null,
  email: "",
  newf: "",
};

export const DP = { id: "", av: 0 as AvatarChoice, img: null as string | null };

/** The recovery code of the account just registered, shown once until the player confirms they saved it; empty otherwise. */
export const REC = { code: "", problem: null as Msg | null };

export const app = {
  scr: "play" as ScreenId,
  run: false,
  stopping: false,
  log: [] as [string, Msg][],
  wz: 0 as number | null,
  host: false,
  info: { launcherVersion: "", emulatorCommit: "", defaultServer: "", updates: false, hosting: false } as AppInfo,
  devices: { inputs: [], outputs: [] } as AudioDevices,
  pub: { online: false, players: null, region: "", network: false, progression: false } as ServerStatus,
  friends: { friends: [], incoming: [], outgoing: [] } as FriendsState,
  /** Who invited the player to a match since the game last started, newest first. */
  invites: [] as string[],
  /** The game's key art, or null for the neutral placeholder. */
  art: null as ArtView | null,
};

/** Game files. prep: 0 not prepared, 1 preparing, 2 ready. err/found: the last rejected folder; note: a rejected counterpart; prepErr: the last preparation error. */
export const F = {
  base: null as DumpInfo | null,
  update: null as DumpInfo | null,
  prep: 0 as 0 | 1 | 2,
  err: null as Msg | null,
  found: null as Msg | null,
  note: null as Msg | null,
  prepErr: null as Msg | null,
  /** Where the chosen PKG files are extracted; null when neither half is a PKG file. */
  inst: null as InstallInfo | null,
  leftovers: [] as Leftover[],
  /** What installing needs at the current location; planErr when the PKG files could not be read. */
  plan: null as InstallPlan | null,
  planErr: null as Msg | null,
  instErr: null as Msg | null,
  prog: null as InstallProgress | null,
  /** When the running installation started and how much was there then, for the time left. */
  progStart: null as { t: number; done: number } | null,
};

/** The half still to choose after the game folder held only one. */
export const missing = (): DumpKind | null => (F.base && !F.update ? "update" : F.update && !F.base ? "base" : null);

export const pkgChosen = (): boolean => F.base?.source === "pkg" || F.update?.source === "pkg";

/** A PKG file is chosen and not extracted yet. */
export const needsInstall = (): boolean => pkgChosen() && !F.inst?.complete;

export const installing = (): boolean => !!F.inst?.running;

export function applyFiles(files: GameFiles): void {
  F.base = files.base;
  F.update = files.update;
  F.prep = files.prepared ? 2 : 0;
  F.inst = files.install;
  F.leftovers = files.leftovers;
}

export const CHECK: ControllerCheck = { profile: "", presses: {} };

export const CACHE: ShaderCacheState = { status: "ready", round: 0, rounds: 8, minRounds: 2, roundSeconds: 0 };

export type UpdatePhase = "idle" | "checking" | "latest" | "available" | "downloading" | "ready" | "failed";

export const UPD = {
  cur: "",
  next: "",
  notes: [] as string[],
  phase: "idle" as UpdatePhase,
  hidden: false,
  prog: null as UpdateProgress | null,
  err: null as Msg | null,
};

export const ts = (): string => new Date().toTimeString().slice(0, 8);

/** A log line: a translatable message, or raw text (emulator output). */
export function log(m: Msg | string, time = ts()): void {
  app.log.push([time, typeof m === "string" ? { message: m } : m]);
  if (app.log.length > 80) app.log.shift();
}

export const logMsg = (key: string, params: Msg["params"] = {}): Msg => ({ key, params });

export const dirtyS = (): boolean => (Object.keys(S) as (keyof Settings)[]).some((k) => S[k] !== SAVED[k]);
export const dirtyP = (): boolean => PR.in && (DP.id !== PR.id || DP.av !== PR.av || DP.img !== PR.img);
export const dirty = (): boolean => dirtyS() || dirtyP();

/** The `{server}` parameter: "official server" or "private server <address>", without the article. */
export const srvName = (): Msg =>
  SAVED.usePrivateServer ? { key: "server.private", params: { address: SAVED.privateServer } } : { key: "server.official" };

/** Changes when the game whose artwork is shown changes (folder, installation, removal). */
export const artKey = (): string => [F.base?.path, F.update?.path, F.inst?.target, F.inst?.complete].join("|");

export const canPlay = (): boolean => F.prep === 2 && PR.in && CACHE.status !== "warming" && !installing();

export function applySession(s: Session): void {
  PR.in = true;
  PR.id = s.onlineId;
  PR.av = s.avatar === "image" ? "img" : s.avatar;
  PR.img = s.image;
  PR.email = s.email;
  Object.assign(DP, { id: PR.id, av: PR.av, img: PR.img });
}

export function setSetting(k: keyof Settings, v: unknown): void {
  (S as unknown as Record<string, unknown>)[k] = v;
}
