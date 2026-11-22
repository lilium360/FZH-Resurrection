import {
  ANISOTROPIC_FILTERS,
  ANTIALIASING_MODES,
  FRAME_RATES,
  GAME_LANGUAGES,
  LAUNCHER_LANGUAGES,
  PIXEL_DENSITIES,
  SHADOW_QUALITIES,
  type PickKind,
  type Settings,
} from "../backend";
import { list, t } from "../i18n";

export type SettingKey = keyof Settings;

/** Label and note come from `meta.<key>.label` / `.note`; `x` is an `option.*` chip. */
export interface SettingMeta {
  t: "sel" | "seg" | "tog" | "sld" | "txt";
  o?: readonly string[];
  a?: readonly [number, number, number];
  u?: string;
  x?: "notVerified";
  st?: boolean;
}

export const M: Partial<Record<SettingKey, SettingMeta>> = {
  launcherLanguage: { t: "sel", o: LAUNCHER_LANGUAGES },
  microphone: { t: "sel" },
  audioOutput: { t: "sel" },
  gameLanguage: { t: "sel", o: GAME_LANGUAGES },
  gripRight: { t: "sld", a: [-0.5, 0.5, 0.01], u: " m" },
  gripUp: { t: "sld", a: [-0.5, 0.5, 0.01], u: " m" },
  gripForward: { t: "sld", a: [-0.2, 0.8, 0.01], u: " m" },
  gripPitch: { t: "sld", a: [-90, 90, 0.5], u: "°" },
  gripYaw: { t: "sld", a: [-90, 90, 0.5], u: "°" },
  gripRoll: { t: "sld", a: [-90, 90, 0.5], u: "°" },
  frameRate: { t: "seg", o: FRAME_RATES },
  asyncShaders: { t: "tog" },
  pixelDensity: { t: "sel", o: PIXEL_DENSITIES },
  sharpness: { t: "sld", a: [0, 1, 0.05] },
  fieldOfView: { t: "sld", a: [50, 100, 1], u: "%" },
  shadowQuality: { t: "seg", o: SHADOW_QUALITIES },
  anisotropicFiltering: { t: "seg", o: ANISOTROPIC_FILTERS, x: "notVerified" },
  antialiasing: { t: "seg", o: ANTIALIASING_MODES, st: true },
  fpsCounter: { t: "tog" },
  frameStats: { t: "tog" },
  modFreeTurn: { t: "tog" },
  usePrivateServer: { t: "tog" },
  privateServer: { t: "txt" },
};

export const SERVICES = {
  network: { ic: "server" },
  progression: { ic: "cpu" },
  relay: { ic: "cast" },
} as const;

export const GRIP_KEYS = ["gripRight", "gripUp", "gripForward", "gripPitch", "gripYaw", "gripRoll"] as const;

export const GRIP: Pick<Settings, (typeof GRIP_KEYS)[number]> = { gripRight: 0, gripUp: 0.07, gripForward: 0.3, gripPitch: 0, gripYaw: 0, gripRoll: 0 };

/** Keys and the `key.*` text of what they do. */
export const KEYS: [string[], string][] = [
  [["F2"], "friendsPanel"],
  [["F10"], "fpsCounter"],
];

/** Screen, icon and, when a group starts there, its `nav.menuGroup.*` key. */
export const MENU: [ScreenId, string, string?][] = [
  ["play", "disc"],
  ["profile", "user", "multiplayer"],
  ["friends", "users"],
  ["graphics", "image", "setup"],
  ["gamepad", "pad"],
  ["mods", "patch"],
  ["settings", "sliders"],
  ["advanced", "tool", "more"],
  ["about", "info"],
];

export type ScreenId = "play" | "profile" | "friends" | "graphics" | "gamepad" | "mods" | "settings" | "advanced" | "about";

export const screenName = (id: ScreenId): string => t(`nav.menu.${id}`);

export const fileName = (k: PickKind): string => t(`file.${k}`);

export const wizardSteps = (): string[] => list("nav.wizardSteps").map(String);

export const WIZARD_LAST = 4;

export const licences = (): [string, string][] => list("licence") as [string, string][];
