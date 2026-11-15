import assert from "node:assert/strict";
import { test } from "node:test";
import {
  buildThemeTokens,
  COMPOSER_BLUR,
  CONTRAST_SLACK,
  FLOAT_BLUR,
  legibleAlpha,
  PANEL_BLUR_MAX,
  PANEL_BLUR_MIN,
  planSurfaces,
  TEXT_TARGET,
  WORST_TONE,
  GLASS_SATURATE
} from "ui/surface-model";
import { contrastRatio, deriveTheme, luminance, mix, paletteById } from "ui/theme-model";
import { combineDim, evaluateTone, makeToneSource, mayBeAnimated, plainTone } from "./backdropTone.ts";

const OBSIDIAN = paletteById("obsidian")!;
const dark = deriveTheme(OBSIDIAN);

/** A w x h RGBA buffer filled by `px(x, y)`. */
function buffer(w: number, h: number, px: (x: number, y: number) => [number, number, number]) {
  const data = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) data.set([...px(x, y), 255], (y * w + x) * 4);
  return data;
}
const toHex = (c: number[]) => `#${c.map((v) => Math.round(v).toString(16).padStart(2, "0")).join("")}`;
const toRgb = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
const env = { bg: dark.bg, dim: 0 };

/* ---- independent reference: CSS saturate() and a direct Gaussian blur with mirrored edges ---- */

function cssSaturate([r, g, b]: number[], s: number): number[] {
  const m = [
    [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
    [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
    [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s]
  ];
  return m.map((row) => Math.max(0, Math.min(255, row[0] * r + row[1] * g + row[2] * b)));
}

function reflect(i: number, n: number): number {
  const period = 2 * n;
  const m = ((i % period) + period) % period;
  return m < n ? m : period - 1 - m;
}

/** Gaussian blur of `sigma` px (separable, kernel to 4 sigma, mirrored edges): the reference kernel. */
function referenceBlur(data: Uint8ClampedArray, w: number, h: number, sigma: number): Float64Array {
  const radius = Math.ceil(sigma * 3);
  const kernel = Array.from({ length: 2 * radius + 1 }, (_, i) => Math.exp(-((i - radius) ** 2) / (2 * sigma * sigma)));
  const total = kernel.reduce((a, b) => a + b, 0);
  const k = kernel.map((v) => v / total);
  const a = new Float64Array(w * h * 3);
  for (let i = 0; i < w * h; i++) for (let c = 0; c < 3; c++) a[i * 3 + c] = data[i * 4 + c];
  const b = new Float64Array(a.length);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++)
      for (let c = 0; c < 3; c++) {
        let acc = 0;
        for (let j = -radius; j <= radius; j++) acc += k[j + radius] * a[(y * w + reflect(x + j, w)) * 3 + c];
        b[(y * w + x) * 3 + c] = acc;
      }
  const out = new Float64Array(a.length);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++)
      for (let c = 0; c < 3; c++) {
        let acc = 0;
        for (let j = -radius; j <= radius; j++) acc += k[j + radius] * b[(reflect(y + j, h) * w + x) * 3 + c];
        out[(y * w + x) * 3 + c] = acc;
      }
  return out;
}

/** Brightest / darkest luminance of `saturate(blur(data))`, what a glass layer of this blur sits on. */
function referenceExtremes(data: Uint8ClampedArray, w: number, h: number, sigma: number) {
  const field = referenceBlur(data, w, h, sigma);
  let max = -1;
  let min = 2;
  for (let i = 0; i < w * h; i++) {
    const l = luminance(toHex(cssSaturate([field[i * 3], field[i * 3 + 1], field[i * 3 + 2]], GLASS_SATURATE)));
    max = Math.max(max, l);
    min = Math.min(min, l);
  }
  return { max, min };
}

/* ---- finding 1: conservative extremes by relative luminance ---- */

test("extremes are by luminance: a #00ff00 half is brighter than a #b8b8b8 half, and the floor holds on it", () => {
  const w = 200;
  const h = 100;
  const src = makeToneSource(buffer(w, h, (x) => (x < w / 2 ? [0, 255, 0] : [184, 184, 184])), w, h, 1);
  const tone = evaluateTone(src, env);
  assert.equal(tone.peak.light, "#00ff00", "green (lum 0.72) is the brightest region, not gray (0.48)");
  const plan = planSurfaces(dark, "tinted", true, { transparency: 100, blur: 28, transparentChat: false, tone });
  const onGreen = mix("#00ff00", plan.tint, plan.shellAlpha);
  assert.ok(contrastRatio(dark.text, onGreen) >= TEXT_TARGET, `body on the green region ${contrastRatio(dark.text, onGreen).toFixed(2)}`);
  const t = buildThemeTokens(OBSIDIAN, "#3b82f6", "tinted", true, { transparency: 100, blur: 28, transparentChat: false, tone });
  assert.ok(contrastRatio(t["--color-text-muted"], onGreen) >= 4.5, "muted text on the green region");
});

test("a small bright patch is not discarded (no percentile trimming)", () => {
  const n = 1000;
  const data = buffer(n, n, (x, y) => (x >= 100 && x < 140 && y >= 100 && y < 140 ? [255, 255, 255] : [10, 10, 10]));
  const tone = evaluateTone(makeToneSource(data, n, n, 1), env);
  assert.equal(tone.peak.light, "#ffffff", "the 40x40 patch is 0.16 % of the pixels and still the brightest region");
  const plan = planSurfaces(dark, "tinted", true, { transparency: 100, blur: 28, transparentChat: false, tone });
  assert.ok(contrastRatio(dark.text, mix("#ffffff", plan.tint, plan.shellAlpha)) >= TEXT_TARGET, "text over the patch stays at 7:1");
});

test("a single bright pixel in a large image counts for unblurred layers", () => {
  const w = 997; // not divisible by any stride
  const h = 613;
  const data = buffer(w, h, (x, y) => (x === 501 && y === 307 ? [255, 255, 255] : [20, 20, 20]));
  assert.equal(evaluateTone(makeToneSource(data, w, h, 1), env).peak.light, "#ffffff");
});

test("random backdrops: the floor from the tone candidates covers the worst pixel (all pixels brute-forced)", () => {
  let seed = 12345;
  const rnd = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  let worstGap = 0;
  for (let trial = 0; trial < 120; trial++) {
    const w = 48;
    const h = 32;
    const palette = Array.from({ length: 6 }, () => [rnd() * 255, rnd() * 255, rnd() * 255].map(Math.round) as [number, number, number]);
    const data = buffer(w, h, (x, y) => palette[(Math.floor(x / 8) + Math.floor(y / 8) * 3) % palette.length]);
    for (const id of ["obsidian", "clay", "cobalt"]) {
      const d = deriveTheme(paletteById(id)!);
      const dim = rnd() < 0.5 ? 0 : 0.4;
      const tone = evaluateTone(makeToneSource(data, w, h, 1), { bg: d.bg, dim });
      const tint = d.light ? mix(d.bg, "#ffffff", 0.45) : d.bg;
      const brute = Math.max(
        ...palette.map((c) => {
          const dimmed = toHex(c.map((v, i) => (1 - dim) * v + dim * toRgb(d.bg)[i]));
          return legibleAlpha(d.text, tint, dimmed, TEXT_TARGET);
        })
      );
      const candidates = d.light ? [tone.peak.dark, ...(tone.peak.darkAlt ?? [])] : [tone.peak.light, ...(tone.peak.lightAlt ?? [])];
      const viaTone = Math.max(...candidates.map((c) => legibleAlpha(d.text, tint, c, TEXT_TARGET + CONTRAST_SLACK)));
      worstGap = Math.max(worstGap, brute - viaTone);
    }
  }
  assert.ok(worstGap <= 0, `the candidates under-estimate the worst pixel's floor by ${worstGap.toFixed(3)} alpha`);
});

/* ---- finding 2: the emitted filters ---- */

test("the soft tone is measured after saturate(): #aa00aa becomes #ff00ff and the floor holds on it", () => {
  const w = 64;
  const h = 64;
  const src = makeToneSource(buffer(w, h, () => [0xaa, 0, 0xaa]), w, h, 1);
  const tone = evaluateTone(src, env);
  assert.equal(tone.soft.light, toHex(cssSaturate([0xaa, 0, 0xaa], GLASS_SATURATE)), "the backdrop as the CSS renders it");
  assert.equal(tone.peak.light, "#aa00aa", "unblurred layers have no filter");
  const opts = { transparency: 100, blur: 28, transparentChat: false, tone };
  const plan = planSurfaces(dark, "glass", true, opts);
  const t = buildThemeTokens(OBSIDIAN, "#3b82f6", "glass", true, opts);
  assert.match(t["--shell-filter"], new RegExp(`saturate\\(${GLASS_SATURATE}\\)`));
  const behind = toHex(cssSaturate([0xaa, 0, 0xaa], GLASS_SATURATE));
  const onPanel = mix(behind, plan.tint, plan.shellAlpha);
  assert.ok(contrastRatio(dark.text, onPanel) >= TEXT_TARGET, `body ${contrastRatio(dark.text, onPanel).toFixed(2)}`);
  assert.ok(contrastRatio(t["--color-text-muted"], onPanel) >= 4.5, `muted ${contrastRatio(t["--color-text-muted"], onPanel).toFixed(2)}`);
});

test("every glass layer's CSS blur is at least as strong as the one the soft tone was measured with", () => {
  // Fixtures with features of every size, at the edges and in the corner (mirrored-edge reference kernel).
  const w = 72;
  const h = 48;
  const patch = (x0: number, y0: number, size: number, color: [number, number, number], ground: [number, number, number]) =>
    buffer(w, h, (x, y) => (x >= x0 && x < x0 + size && y >= y0 && y < y0 + size ? color : ground));
  const fixtures: [string, Uint8ClampedArray][] = [
    ["corner speck 3px", patch(0, 0, 3, [255, 255, 255], [10, 10, 10])],
    ["corner patch 24px", patch(0, 0, 24, [255, 255, 255], [10, 10, 10])],
    ["edge patch 12px", patch(0, 18, 12, [255, 255, 255], [10, 10, 10])],
    ["centre patch 20px", patch(26, 14, 20, [255, 255, 255], [10, 10, 10])],
    ["dark patch on light", patch(0, 0, 18, [0, 0, 0], [240, 240, 240])],
    ["saturated green patch", patch(52, 26, 20, [0, 255, 0], [30, 30, 90])],
    ["checker 2px", buffer(w, h, (x, y) => (((x >> 1) + (y >> 1)) % 2 ? [255, 255, 255] : [0, 0, 0]))],
    ["checker 9px", buffer(w, h, (x, y) => ((Math.floor(x / 9) + Math.floor(y / 9)) % 2 ? [255, 255, 255] : [0, 0, 0]))]
  ];
  const sigmas = new Set<number>();
  for (const b of [PANEL_BLUR_MIN, 28, PANEL_BLUR_MAX]) {
    sigmas.add(b); // shell panel
    sigmas.add(Math.round(b * COMPOSER_BLUR)); // composer
    sigmas.add(Math.round(b * FLOAT_BLUR)); // floating surfaces
  }
  for (const [name, data] of fixtures) {
    const tone = evaluateTone(makeToneSource(data, w, h, 1), env);
    const lightL = luminance(tone.soft.light);
    const darkL = luminance(tone.soft.dark);
    for (const sigma of sigmas) {
      const ref = referenceExtremes(data, w, h, sigma);
      assert.ok(lightL >= ref.max - 0.002, `${name} @ blur ${sigma}px: brightest ${lightL.toFixed(3)} < reference ${ref.max.toFixed(3)}`);
      assert.ok(darkL <= ref.min + 0.002, `${name} @ blur ${sigma}px: darkest ${darkL.toFixed(3)} > reference ${ref.min.toFixed(3)}`);
    }
  }
});

test("the old 32 px cell average is NOT conservative: a 20 px patch at the composer's smallest blur", () => {
  // Documents why cells were replaced by the real kernel: the cell average dilutes a 20 px patch to
  // (20/32)^2 = 39 %, the 9 px Gaussian at its centre keeps ~ 53 %.
  const w = 120;
  const h = 80;
  const data = buffer(w, h, (x, y) => (x >= 50 && x < 70 && y >= 30 && y < 50 ? [255, 255, 255] : [0, 0, 0]));
  const ref = referenceExtremes(data, w, h, Math.round(PANEL_BLUR_MIN * COMPOSER_BLUR));
  const cellAverage = 255 * (20 / 32) ** 2;
  assert.ok(luminance(toHex([cellAverage, cellAverage, cellAverage])) < ref.max, "a cell average under-reports this patch");
});

/* ---- dim layers, plain backdrops, unsupported sources ---- */

test("dim layers: evaluating with dim equals evaluating the pre-dimmed pixels", () => {
  const w = 60;
  const h = 40;
  const raw = buffer(w, h, (x, y) => [(x * 4) & 255, (y * 6) & 255, ((x + y) * 3) & 255]);
  const bg = toRgb(dark.bg);
  const dim = combineDim(0.5, 0.2); // two layers: 1 - 0.5 * 0.8 = 0.6
  assert.equal(dim, 0.6);
  const dimmed = buffer(w, h, (x, y) => [0, 1, 2].map((c) => (1 - dim) * raw[(y * w + x) * 4 + c] + dim * bg[c]) as [number, number, number]);
  const a = evaluateTone(makeToneSource(raw, w, h, 1), { bg: dark.bg, dim });
  const b = evaluateTone(makeToneSource(dimmed, w, h, 1), { bg: dark.bg, dim: 0 });
  for (const key of ["peak", "soft"] as const) {
    for (const side of ["light", "dark"] as const) {
      const d = Math.abs(luminance(a[key][side]) - luminance(b[key][side]));
      assert.ok(d < 0.01, `${key}.${side} differs by ${d.toFixed(4)}`);
    }
  }
});

test("a source with translucent pixels (or an empty one) keeps the worst case", () => {
  const data = buffer(8, 8, () => [10, 10, 10]);
  data[3] = 128;
  assert.deepEqual(evaluateTone(makeToneSource(data, 8, 8, 1), env), WORST_TONE);
});

test("textured backdrops report their local contrast; smooth ones do not", () => {
  const dots = buffer(64, 64, (x, y) => (x % 4 === 0 && y % 4 === 0 ? [255, 255, 255] : [16, 16, 16]));
  const smooth = buffer(64, 64, (x) => [60 + x / 2, 60 + x / 2, 60 + x / 2]); // a gentle gradient
  const a = evaluateTone(makeToneSource(dots, 64, 64, 1), env).detail;
  const b = evaluateTone(makeToneSource(smooth, 64, 64, 1), env).detail;
  assert.ok(a && contrastRatio(a.light, a.dark) > 5, "dots are busy");
  assert.ok(b && contrastRatio(b.light, b.dark) < 1.5, "a gradient is not");
});

test("no backdrop: the layout gradient's stops, saturated for the glass layers", () => {
  const t = plainTone("#111217", "#27282d");
  assert.deepEqual(t.peak, { light: "#27282d", dark: "#111217" });
  assert.equal(luminance(t.soft.light) >= luminance(t.peak.light) - 0.01, true);
});

test("animated images cannot be bounded", () => {
  assert.equal(mayBeAnimated("data:image/gif;base64,R0lGODlhAQABAAAAACw="), true);
  assert.equal(mayBeAnimated("file:///x/y.GIF"), true);
  assert.equal(mayBeAnimated("data:image/jpeg;base64,/9j/4AAQSkZJRg=="), false);
  const webpAnim = Buffer.concat([Buffer.from("RIFF"), Buffer.alloc(4), Buffer.from("WEBPVP8X"), Buffer.alloc(4), Buffer.from([0x02]), Buffer.alloc(40)]);
  const webpStill = Buffer.concat([Buffer.from("RIFF"), Buffer.alloc(4), Buffer.from("WEBPVP8X"), Buffer.alloc(4), Buffer.from([0x00]), Buffer.alloc(40)]);
  assert.equal(mayBeAnimated(`data:image/webp;base64,${webpAnim.toString("base64")}`), true);
  assert.equal(mayBeAnimated(`data:image/webp;base64,${webpStill.toString("base64")}`), false);
  const apng = Buffer.concat([Buffer.from("\x89PNG\r\n\x1a\n"), Buffer.alloc(8), Buffer.from("acTL"), Buffer.alloc(20)]);
  const png = Buffer.concat([Buffer.from("\x89PNG\r\n\x1a\n"), Buffer.alloc(8), Buffer.from("IDAT"), Buffer.alloc(20)]);
  assert.equal(mayBeAnimated(`data:image/png;base64,${apng.toString("base64")}`), true);
  assert.equal(mayBeAnimated(`data:image/png;base64,${png.toString("base64")}`), false);
});
