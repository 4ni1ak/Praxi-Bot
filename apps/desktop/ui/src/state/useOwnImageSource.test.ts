import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { coverRect, makeToneSource, evaluateTone } from "../lib/backdropTone.ts";
import { RESAMPLE_DEBOUNCE_MS, usableSample, type Sampled } from "./useOwnImageSource.ts";

const source = makeToneSource(new Uint8ClampedArray([10, 10, 10, 255]), 1, 1, 1);
const sampled: Sampled = { url: "data:image/png;base64,AAAA", width: 1400, height: 900, source };

test("a sample is valid only for its image at the size it was taken at", () => {
  assert.equal(usableSample(sampled, sampled.url, { width: 1400, height: 900 }), source);
  assert.equal(usableSample(sampled, sampled.url, { width: 1401, height: 900 }), null, "window resized: stale at once");
  assert.equal(usableSample(sampled, sampled.url, { width: 1400, height: 899 }), null);
  assert.equal(usableSample(sampled, "data:image/png;base64,BBBB", { width: 1400, height: 900 }), null, "another image");
  assert.equal(usableSample(null, sampled.url, { width: 1400, height: 900 }), null, "nothing sampled yet: worst case");
  assert.equal(usableSample({ ...sampled, source: null }, sampled.url, { width: 1400, height: 900 }), null, "unreadable image: worst case");
});

test("the crop of a wide image changes with the window, so the floor must be taken again after a resize", () => {
  // A 3000 x 1000 image with a dark middle and bright sides.
  const crop = (w: number, h: number) => {
    const r = coverRect(3000, 1000, w, h);
    return { left: -r.x / r.width, right: (-r.x + w) / r.width }; // visible share of the image, left..right
  };
  const square = crop(900, 900); // cover: height fits, the sides are cropped
  const wide = crop(1800, 600); // same ratio as the image: everything is visible
  assert.ok(square.left > 0.3 && square.right < 0.7, `square window sees only the middle ${square.left.toFixed(2)}..${square.right.toFixed(2)}`);
  assert.ok(wide.left < 0.001 && wide.right > 0.999, "a wide window sees the bright sides too");
  // The two crops really have different worst cases (dark middle vs bright sides).
  const band = (x0: number, x1: number) => {
    const w = 300;
    const data = new Uint8ClampedArray(w * 10 * 4);
    for (let y = 0; y < 10; y++) for (let x = 0; x < w; x++) {
      const u = x0 + ((x1 - x0) * x) / w;
      data.set(u < 0.3 || u > 0.7 ? [255, 255, 255, 255] : [12, 12, 12, 255], (y * w + x) * 4);
    }
    return evaluateTone(makeToneSource(data, w, 10, 1), { bg: "#111217", dim: 0 }).peak.light;
  };
  assert.equal(band(square.left, square.right), "#0c0c0c");
  assert.equal(band(wide.left, wide.right), "#ffffff");
});

test("the hook resamples on resize (debounced) and the appearance hook uses it for own images", () => {
  assert.ok(RESAMPLE_DEBOUNCE_MS >= 100 && RESAMPLE_DEBOUNCE_MS <= 400, "debounced, not per event and not sluggish");
  const hook = readFileSync(new URL("./useOwnImageSource.ts", import.meta.url), "utf8");
  assert.match(hook, /addEventListener\("resize"/);
  assert.match(hook, /setTimeout\(\(\) => setSize\(read\(\)\), RESAMPLE_DEBOUNCE_MS\)/);
  const apply = readFileSync(new URL("./useApplyAppearance.ts", import.meta.url), "utf8");
  assert.match(apply, /useOwnImageSource\(imageUrl\)/);
  assert.doesNotMatch(apply, /sampleImageTone/, "no one-shot per-URL sampling left");
});
