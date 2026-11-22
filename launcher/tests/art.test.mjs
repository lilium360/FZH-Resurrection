import assert from "node:assert/strict";
import { test } from "node:test";
import { contentBox } from "../src/ui/art.ts";

const image = (w, h, bg, paint) => {
  const d = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const c = paint(x, y) ?? bg;
      d.set([...c, 255], (y * w + x) * 4);
    }
  return d;
};

test("a logo on a plain background is trimmed to its box, noise ignored", () => {
  const d = image(100, 50, [0, 0, 0], (x, y) => (x >= 20 && x < 80 && y >= 20 && y < 31 ? [200, 150, 30] : (x + y) % 7 === 0 ? [9, 6, 11] : null));
  const { bg, ...box } = contentBox(d, 100, 50);
  assert.deepEqual(box, { x: 20, y: 20, w: 60, h: 11 });
  assert.ok(bg.every((c) => c < 10));
});

test("art without a uniform border has no box", () => {
  const d = image(60, 40, [0, 0, 0], (x, y) => [x * 4, y * 6, 100]);
  assert.equal(contentBox(d, 60, 40), null);
});

test("a blank image has no content", () => {
  assert.equal(contentBox(image(10, 10, [30, 30, 30], () => null), 10, 10), null);
});
