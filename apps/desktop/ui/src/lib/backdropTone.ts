/**
 * Backdrop tone (card 244-E): the lightest and darkest colours the backdrop shows, so the surface
 * model can make panels exactly as transparent as legibility allows.  The pixel maths is pure (and
 * tested); the sampling helpers read a canvas or an image once per change, never per frame.
 */
import type { BackdropTone, ToneExtremes } from "ui/surface-model";
import { luminance, mix } from "ui/theme-model";

/** Size of a cell the glass blur averages over (window px); matches `PANEL_BLUR_MIN`'s window. */
export const SOFT_CELL_PX = 32;
/** Ignore the brightest / darkest 0.5 % of pixels (stray anti-aliasing, single sparkles). */
const PEAK_PERCENTILE = 0.005;
/** Pixels read at most from one source (larger sources are read with a stride). */
const MAX_SAMPLES = 120_000;

const hex = (r: number, g: number, b: number) =>
  `#${[r, g, b].map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0")).join("")}`;

/** Cheap luma bucket 0..255 (ranking only; exact WCAG luminance is used for the winners). */
const luma = (r: number, g: number, b: number) => (r * 54 + g * 183 + b * 19) >> 8;

/**
 * Tone of an RGBA pixel buffer `w` x `h`.  `cell` is the soft-cell size in buffer pixels (what
 * `SOFT_CELL_PX` window pixels are in this buffer).  Fully transparent pixels are skipped (they
 * show the layout background, which the caller accounts for).  One pass, a 256-bucket luma
 * histogram and per-cell accumulators: no per-pixel allocation.
 */
export function toneFromPixels(data: ArrayLike<number>, w: number, h: number, cell: number): BackdropTone {
  const stride = Math.max(1, Math.ceil(Math.sqrt((w * h) / MAX_SAMPLES)));
  const size = Math.max(stride, Math.round(cell));
  const cols = Math.ceil(w / size);
  const rows = Math.ceil(h / size);
  const cells = cols * rows;
  const sum = new Float64Array(cells * 3);
  const count = new Uint32Array(cells);
  const lo = new Int32Array(cells).fill(-1);
  const hi = new Int32Array(cells).fill(-1);
  const loL = new Int16Array(cells).fill(256);
  const hiL = new Int16Array(cells).fill(-1);
  const hist = new Uint32Array(256);
  const histPx = new Int32Array(256).fill(-1);
  let total = 0;
  for (let y = 0; y < h; y += stride) {
    const row = Math.floor(y / size) * cols;
    for (let x = 0; x < w; x += stride) {
      const i = (y * w + x) * 4;
      if (data[i + 3] === 0) continue;
      const r = data[i];
      const g = data[i + 1];
      const b = data[i + 2];
      const l = luma(r, g, b);
      hist[l]++;
      histPx[l] = i;
      total++;
      const c = row + Math.floor(x / size);
      sum[c * 3] += r;
      sum[c * 3 + 1] += g;
      sum[c * 3 + 2] += b;
      count[c]++;
      if (l < loL[c]) {
        loL[c] = l;
        lo[c] = i;
      }
      if (l > hiL[c]) {
        hiL[c] = l;
        hi[c] = i;
      }
    }
  }
  if (!total) {
    const none = { light: "#ffffff", dark: "#000000" };
    return { peak: none, soft: none };
  }
  const at = (i: number) => hex(data[i], data[i + 1], data[i + 2]);
  // Peak: walk the histogram from each end until the percentile is passed.
  const skip = Math.floor(total * PEAK_PERCENTILE);
  const fromEnd = (dir: 1 | -1) => {
    let seen = 0;
    for (let k = dir === 1 ? 0 : 255; k >= 0 && k <= 255; k += dir) {
      seen += hist[k];
      if (seen > skip && histPx[k] >= 0) return at(histPx[k]);
    }
    return dir === 1 ? "#000000" : "#ffffff";
  };
  // Soft: extremes of the cell averages; detail: the cell at the 90th percentile of local contrast.
  let softLight = "";
  let softDark = "";
  let softLightL = -1;
  let softDarkL = 2;
  const local: { ratio: number; c: number }[] = [];
  for (let c = 0; c < cells; c++) {
    const n = count[c];
    if (!n) continue;
    const avg = hex(sum[c * 3] / n, sum[c * 3 + 1] / n, sum[c * 3 + 2] / n);
    const L = luminance(avg);
    if (L > softLightL) {
      softLightL = L;
      softLight = avg;
    }
    if (L < softDarkL) {
      softDarkL = L;
      softDark = avg;
    }
    if (hiL[c] > loL[c]) local.push({ ratio: hiL[c] - loL[c], c });
    else local.push({ ratio: 0, c });
  }
  local.sort((a, b) => a.ratio - b.ratio);
  const p90 = local[Math.min(local.length - 1, Math.floor(local.length * 0.9))];
  return {
    peak: { light: fromEnd(-1), dark: fromEnd(1) },
    soft: { light: softLight, dark: softDark },
    ...(p90 ? { detail: { light: at(hi[p90.c]), dark: at(lo[p90.c]) } } : {})
  };
}

/** The tone after a layer of `color` at `amount` (0..1) is laid over the backdrop (the dim layers). */
export function dimTone(tone: BackdropTone, color: string, amount: number): BackdropTone {
  const a = Math.min(1, Math.max(0, Number.isFinite(amount) ? amount : 0));
  const ext = (e: ToneExtremes): ToneExtremes => ({ light: mix(e.light, color, a), dark: mix(e.dark, color, a) });
  return { peak: ext(tone.peak), soft: ext(tone.soft), ...(tone.detail ? { detail: ext(tone.detail) } : {}) };
}

/** Tone of the plain layout background (no backdrop): its two gradient stops. */
export function plainTone(bg: string, bg2: string): BackdropTone {
  const [dark, light] = luminance(bg) <= luminance(bg2) ? [bg, bg2] : [bg2, bg];
  return { peak: { light, dark }, soft: { light, dark } };
}

/* ---------- sampling (DOM) ---------- */

/** Tone of a drawn canvas that covers the window; null when it cannot be read. */
export function sampleCanvasTone(canvas: HTMLCanvasElement, windowWidth: number): BackdropTone | null {
  try {
    const ctx = canvas.getContext("2d", { willReadFrequently: false });
    if (!ctx || !canvas.width || !canvas.height) return null;
    const { data } = ctx.getImageData(0, 0, canvas.width, canvas.height);
    const cell = (SOFT_CELL_PX * canvas.width) / Math.max(1, windowWidth);
    return toneFromPixels(data, canvas.width, canvas.height, cell);
  } catch {
    // A tainted canvas (cross-origin image) cannot be read: the caller assumes the worst case.
    return null;
  }
}

/** Tone of an image laid out like `background-size: cover` on a window; null when it cannot be read. */
export function sampleImageTone(url: string, windowWidth: number, windowHeight: number): Promise<BackdropTone | null> {
  return new Promise((resolve) => {
    if (typeof document === "undefined" || !url) {
      resolve(null);
      return;
    }
    const img = new Image();
    img.decoding = "async";
    img.onload = () => {
      try {
        const w = Math.max(1, Math.round(windowWidth / 4));
        const h = Math.max(1, Math.round(windowHeight / 4));
        const cv = document.createElement("canvas");
        cv.width = w;
        cv.height = h;
        const ctx = cv.getContext("2d");
        if (!ctx || !img.naturalWidth || !img.naturalHeight) {
          resolve(null);
          return;
        }
        const k = Math.max(w / img.naturalWidth, h / img.naturalHeight);
        const dw = img.naturalWidth * k;
        const dh = img.naturalHeight * k;
        ctx.drawImage(img, (w - dw) / 2, (h - dh) / 2, dw, dh);
        resolve(sampleCanvasTone(cv, windowWidth));
      } catch {
        resolve(null);
      }
    };
    img.onerror = () => resolve(null);
    img.src = url;
  });
}

/* ---------- the drawn scene's tone (published by <Backdrop>) ---------- */

type Listener = () => void;
let sceneTone: { scene: string; tone: BackdropTone } | null = null;
const listeners = new Set<Listener>();

/** Raw (undimmed) tone of the scene currently drawn, or null. */
export function getSceneTone(scene: string): BackdropTone | null {
  return sceneTone && sceneTone.scene === scene ? sceneTone.tone : null;
}

export function publishSceneTone(scene: string, tone: BackdropTone | null): void {
  const next = tone ? { scene, tone } : null;
  if (JSON.stringify(next) === JSON.stringify(sceneTone)) return;
  sceneTone = next;
  for (const l of listeners) l();
}

export function subscribeSceneTone(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
