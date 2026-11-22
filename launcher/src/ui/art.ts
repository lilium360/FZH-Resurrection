/** How the banner shows the game's key art. `fit`: the art had a plain border, trimmed away; the
 * content is shown whole on its background colour. Without `fit` the art fills the banner. */
export interface ArtView {
  url: string;
  fit: { bg: string; w: number; h: number } | null;
}

/** Channel difference still counted as the background (compression noise). */
const TOLERANCE = 28;
/** A trimmed box at least this share of both sides counts as full-bleed art. */
const FULL_BLEED = 0.92;

export const load = (url: string): Promise<HTMLImageElement> =>
  new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = url;
  });

/** The content's bounding box: what differs from the colour of the four corners. */
export function contentBox(data: Uint8ClampedArray, w: number, h: number): { x: number; y: number; w: number; h: number; bg: [number, number, number] } | null {
  const px = (x: number, y: number) => {
    const i = (y * w + x) * 4;
    return [data[i] ?? 0, data[i + 1] ?? 0, data[i + 2] ?? 0];
  };
  const corners = [px(0, 0), px(w - 1, 0), px(0, h - 1), px(w - 1, h - 1)];
  const bg = [0, 1, 2].map((c) => Math.round(corners.reduce((s, p) => s + (p[c] ?? 0), 0) / 4)) as [number, number, number];
  if (corners.some((p) => p.some((v, c) => Math.abs(v - bg[c]!) > TOLERANCE))) return null;
  let x0 = w,
    y0 = h,
    x1 = -1,
    y1 = -1;
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 4;
      if (Math.abs(data[i]! - bg[0]) > TOLERANCE || Math.abs(data[i + 1]! - bg[1]) > TOLERANCE || Math.abs(data[i + 2]! - bg[2]) > TOLERANCE) {
        if (x < x0) x0 = x;
        if (x > x1) x1 = x;
        if (y < y0) y0 = y;
        if (y > y1) y1 = y;
      }
    }
  if (x1 < 0) return null;
  return { x: x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1, bg };
}

/** Turns the PNG bytes into what the banner shows: the trimmed content on its background, or the whole art. */
export async function prepareArt(bytes: ArrayBuffer): Promise<ArtView> {
  const whole = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
  try {
    const img = await load(whole);
    const { naturalWidth: W, naturalHeight: H } = img;
    const scale = Math.min(1, 480 / W);
    const sw = Math.max(1, Math.round(W * scale)),
      sh = Math.max(1, Math.round(H * scale));
    const small = document.createElement("canvas");
    small.width = sw;
    small.height = sh;
    const sctx = small.getContext("2d", { willReadFrequently: true });
    if (!sctx) return { url: whole, fit: null };
    sctx.drawImage(img, 0, 0, sw, sh);
    const box = contentBox(sctx.getImageData(0, 0, sw, sh).data, sw, sh);
    if (!box || (box.w >= FULL_BLEED * sw && box.h >= FULL_BLEED * sh)) return { url: whole, fit: null };
    const margin = 2;
    const x = Math.max(0, Math.floor((box.x - margin) / scale)),
      y = Math.max(0, Math.floor((box.y - margin) / scale));
    const w = Math.min(W - x, Math.ceil((box.w + 2 * margin) / scale)),
      h = Math.min(H - y, Math.ceil((box.h + 2 * margin) / scale));
    const crop = document.createElement("canvas");
    crop.width = w;
    crop.height = h;
    crop.getContext("2d")?.drawImage(img, x, y, w, h, 0, 0, w, h);
    const blob = await new Promise<Blob | null>((r) => crop.toBlob(r, "image/png"));
    if (!blob) return { url: whole, fit: null };
    URL.revokeObjectURL(whole);
    return { url: URL.createObjectURL(blob), fit: { bg: `rgb(${box.bg.join(",")})`, w, h } };
  } catch {
    return { url: whole, fit: null };
  }
}
