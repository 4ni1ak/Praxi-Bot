import assert from "node:assert/strict";
import { test } from "node:test";
import { dimTone, plainTone, toneFromPixels } from "./backdropTone.ts";

/** A w x h RGBA buffer filled by `px(x, y)`. */
function buffer(w: number, h: number, px: (x: number, y: number) => [number, number, number, number]) {
  const data = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) data.set(px(x, y), (y * w + x) * 4);
  return data;
}

test("a flat backdrop has the same peak and soft extremes", () => {
  const tone = toneFromPixels(buffer(40, 20, () => [74, 123, 208, 255]), 40, 20, 8);
  assert.deepEqual(tone.peak, { light: "#4a7bd0", dark: "#4a7bd0" });
  assert.deepEqual(tone.soft, tone.peak);
  assert.deepEqual(tone.detail, tone.peak, "no texture");
});

test("fine bright dots count for unblurred layers but average out under the glass blur", () => {
  // Dark ground with a white dot on every 4th pixel (a dotted scene).
  const data = buffer(64, 64, (x, y) => (x % 4 === 0 && y % 4 === 0 ? [255, 255, 255, 255] : [16, 16, 16, 255]));
  const tone = toneFromPixels(data, 64, 64, 16);
  assert.equal(tone.peak.light, "#ffffff", "the dots are the peak");
  assert.equal(tone.peak.dark, "#101010");
  assert.equal(tone.soft.light, "#1f1f1f", "a 16 px cell averages 1 white in 16 pixels");
  assert.deepEqual(tone.detail, { light: "#ffffff", dark: "#101010" }, "the texture inside one cell");
});

test("a smooth gradient has no texture worth a scrim", () => {
  const data = buffer(64, 8, (x) => [x * 4, x * 4, x * 4, 255]);
  const { detail } = toneFromPixels(data, 64, 8, 4);
  assert.ok(detail);
  assert.ok(detail.light !== detail.dark && parseInt(detail.light.slice(1, 3), 16) - parseInt(detail.dark.slice(1, 3), 16) <= 12);
});

test("transparent pixels are ignored", () => {
  const data = buffer(10, 10, (x) => (x < 5 ? [0, 0, 0, 0] : [200, 200, 200, 255]));
  assert.deepEqual(toneFromPixels(data, 10, 10, 5).peak, { light: "#c8c8c8", dark: "#c8c8c8" });
});

test("dim layers pull both extremes toward the theme background", () => {
  const raw = { peak: { light: "#ffffff", dark: "#000000" }, soft: { light: "#ffffff", dark: "#000000" } };
  assert.deepEqual(dimTone(raw, "#111217", 0).peak, raw.peak);
  assert.deepEqual(dimTone(raw, "#111217", 1).peak, { light: "#111217", dark: "#111217" });
  assert.equal(dimTone(raw, "#000000", 0.5).soft.light, "#808080");
  assert.deepEqual(dimTone(raw, "#000000", Number.NaN), raw, "a malformed dim is no dim");
});

test("no backdrop: the layout gradient's stops are the extremes", () => {
  assert.deepEqual(plainTone("#111217", "#27282d").peak, { light: "#27282d", dark: "#111217" });
  assert.deepEqual(plainTone("#f4f1ed", "#e0d8cf").peak, { light: "#f4f1ed", dark: "#e0d8cf" });
});
