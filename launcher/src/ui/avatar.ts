import { AVATAR_MAX_BYTES, AVATAR_SIDE } from "../backend";
import { load } from "./art";
import { AV, ini } from "./components";
import type { AvatarChoice } from "./state";

function canvas(): [HTMLCanvasElement, CanvasRenderingContext2D] {
  const c = document.createElement("canvas");
  c.width = c.height = AVATAR_SIDE;
  const ctx = c.getContext("2d", { alpha: false });
  if (!ctx) throw new Error("canvas unavailable");
  return [c, ctx];
}

/** Decoded size of a base64 data URL. */
export const dataUrlBytes = (url: string): number => {
  const b64 = url.slice(url.indexOf(",") + 1);
  return Math.floor((b64.length * 3) / 4) - (b64.endsWith("==") ? 2 : b64.endsWith("=") ? 1 : 0);
};

/** Start and end of a CSS `linear-gradient(<deg>)` across a square of side `s`. */
export function gradientLine(deg: number, s: number): [number, number, number, number] {
  const r = (deg * Math.PI) / 180;
  const dx = Math.sin(r),
    dy = -Math.cos(r);
  const half = ((Math.abs(dx) + Math.abs(dy)) * s) / 2;
  return [s / 2 - dx * half, s / 2 - dy * half, s / 2 + dx * half, s / 2 + dy * half];
}

/** A preset as the launcher draws it: the gradient and colours of its CSS class, with the initials. */
async function presetPng(id: string, index: number): Promise<string> {
  const el = document.createElement("span");
  el.className = `av lg ${AV[index] ?? AV[0]}`;
  el.style.cssText = "position:fixed;visibility:hidden";
  document.body.append(el);
  const cs = getComputedStyle(el);
  const stops = [...cs.backgroundImage.matchAll(/rgba?\([^)]*\)/g)].map((m) => m[0]);
  const deg = Number(/(-?[\d.]+)deg/.exec(cs.backgroundImage)?.[1] ?? 180);
  const px = Math.round((parseFloat(cs.fontSize) / parseFloat(cs.width)) * AVATAR_SIDE);
  const font = `${cs.fontWeight} ${px}px ${cs.fontFamily}`;
  const color = cs.color;
  el.remove();
  await document.fonts.load(font).catch(() => undefined);
  const [c, ctx] = canvas();
  const g = ctx.createLinearGradient(...gradientLine(deg, AVATAR_SIDE));
  stops.forEach((s, i) => g.addColorStop(stops.length > 1 ? i / (stops.length - 1) : 0, s));
  ctx.fillStyle = stops.length ? g : "#1d1d22";
  ctx.fillRect(0, 0, AVATAR_SIDE, AVATAR_SIDE);
  ctx.font = font;
  ctx.fillStyle = color;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(ini(id), AVATAR_SIDE / 2, AVATAR_SIDE / 2);
  return c.toDataURL("image/png");
}

/** A picture the player chose, cropped to the centre square and scaled to 256×256; null when the PNG stays over 256 KB. */
export async function picturePng(file: Blob): Promise<string | null> {
  const url = URL.createObjectURL(file);
  try {
    const img = await load(url);
    const { naturalWidth: w, naturalHeight: h } = img;
    const side = Math.min(w, h);
    const [c, ctx] = canvas();
    ctx.fillStyle = "#0a0a0b";
    ctx.fillRect(0, 0, AVATAR_SIDE, AVATAR_SIDE);
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(img, (w - side) / 2, (h - side) / 2, side, side, 0, 0, AVATAR_SIDE, AVATAR_SIDE);
    const png = c.toDataURL("image/png");
    return dataUrlBytes(png) <= AVATAR_MAX_BYTES ? png : null;
  } finally {
    URL.revokeObjectURL(url);
  }
}

/** The PNG of the avatar the launcher shows for `o`. */
export const avatarPng = (o: { id: string; av: AvatarChoice; img: string | null }): Promise<string> =>
  o.av === "img" && o.img ? Promise.resolve(o.img) : presetPng(o.id, typeof o.av === "number" ? o.av : 0);
