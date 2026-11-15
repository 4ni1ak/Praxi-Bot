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
const MAX_SAMPLES = 400_000;

const hex = (r: number, g: number, b: number) =>
  `#${[r, g, b].map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0")).join("")}`;

interface Px {
  c: string;
  l: number;
}

function extremes(list: Px[], percentile: number): ToneExtremes {
  if (!list.length) return { light: "#ffffff", dark: "#000000" };
  const sorted = [...list].sort((a, b) => a.l - b.l);
  const k = Math.min(sorted.length - 1, Math.floor(sorted.length * percentile));
  return { dark: sorted[k].c, light: sorted[sorted.length - 1 - k].c };
}

/**
 * Tone of an RGBA pixel buffer `w` x `h`.  `cell` is the soft-cell size in buffer pixels (what
 * `SOFT_CELL_PX` window pixels are in this buffer).  Fully transparent pixels are skipped (they
 * show the layout background, which the caller accounts for).
 */
export function toneFromPixels(data: ArrayLike<number>, w: number, h: number, cell: number): BackdropTone {
  const stride = Math.max(1, Math.ceil(Math.sqrt((w * h) / MAX_SAMPLES)));
  const peak: Px[] = [];
  for (let y = 0; y < h; y += stride) {
    for (let x = 0; x < w; x += stride) {
      const i = (y * w + x) * 4;
      if (data[i + 3] === 0) continue;
      const c = hex(data[i], data[i + 1], data[i + 2]);
      peak.push({ c, l: luminance(c) });
    }
  }
  const size = Math.max(1, Math.round(cell));
  const soft: Px[] = [];
  for (let cy = 0; cy < h; cy += size) {
    for (let cx = 0; cx < w; cx += size) {
      let r = 0;
      let g = 0;
      let b = 0;
      let n = 0;
      for (let y = cy; y < Math.min(h, cy + size); y += stride) {
        for (let x = cx; x < Math.min(w, cx + size); x += stride) {
          const i = (y * w + x) * 4;
          if (data[i + 3] === 0) continue;
          r += data[i];
          g += data[i + 1];
          b += data[i + 2];
          n++;
        }
      }
      if (n) {
        const c = hex(r / n, g / n, b / n);
        soft.push({ c, l: luminance(c) });
      }
    }
  }
  return { peak: extremes(peak, PEAK_PERCENTILE), soft: extremes(soft, 0) };
}

/** The tone after a layer of `color` at `amount` (0..1) is laid over the backdrop (the dim layers). */
export function dimTone(tone: BackdropTone, color: string, amount: number): BackdropTone {
  const a = Math.min(1, Math.max(0, Number.isFinite(amount) ? amount : 0));
  const ext = (e: ToneExtremes): ToneExtremes => ({ light: mix(e.light, color, a), dark: mix(e.dark, color, a) });
  return { peak: ext(tone.peak), soft: ext(tone.soft) };
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
