/**
 * Panel surfaces (card 244-E): solid, tinted and glass shell panels, a transparent chat area,
 * and the legibility floor that keeps text readable over any backdrop.
 *
 * The idea: every translucent layer that carries text has an opacity of at least the
 * "legibility floor" — the smallest alpha at which the theme's text still reaches
 * `TEXT_TARGET` against the worst colour the backdrop can show behind that layer.  The user's
 * transparency slider moves the alpha between a comfortable maximum and that floor, so 100 %
 * transparency means "as clear as readability allows over this backdrop".  The muted / status /
 * accent text colours are then re-derived so they reach 4.5:1 on every composite.
 *
 * Pure functions only (no DOM) so `node --test` and `check:themes` can run them.
 */
import {
  buildTokens,
  contrastRatio,
  deriveTheme,
  mix,
  pushContrast,
  type DerivedTheme,
  type ThemeDef,
  type UiStyle
} from "./themeModel.ts";

/* ---------- settings ---------- */

/** Transparency slider (percent): 0 = the most opaque the style allows, 100 = the legibility floor. */
export const PANEL_TRANSPARENCY_MIN = 0;
export const PANEL_TRANSPARENCY_MAX = 100;
export const PANEL_TRANSPARENCY_DEFAULT = 50;
/**
 * Blur radius of the glass (px).  Below 12 the backdrop's detail shows through sharply enough to
 * fight the text (and the tone sampling, which averages ~32 px cells, would no longer be valid);
 * above 48 WebView2 spends noticeably more GPU time for no visible gain.
 */
export const PANEL_BLUR_MIN = 12;
export const PANEL_BLUR_MAX = 48;
export const PANEL_BLUR_DEFAULT = 28;

export interface SurfaceOptions {
  /** 0..100, see `PANEL_TRANSPARENCY_*`. */
  transparency: number;
  /** px, see `PANEL_BLUR_*`. */
  blur: number;
  /** The center chat area drops its own background; the backdrop shows behind the conversation. */
  transparentChat: boolean;
  /** Sampled colours of the backdrop (after its dim layer); `undefined` = assume the worst case. */
  tone?: BackdropTone;
}

export const DEFAULT_SURFACE: SurfaceOptions = {
  transparency: PANEL_TRANSPARENCY_DEFAULT,
  blur: PANEL_BLUR_DEFAULT,
  transparentChat: false
};

const clampNumber = (v: unknown, min: number, max: number, fallback: number): number => {
  const n = typeof v === "number" ? v : typeof v === "string" && v.trim() !== "" ? Number(v) : NaN;
  return Number.isFinite(n) ? Math.min(max, Math.max(min, Math.round(n))) : fallback;
};

export const parsePanelTransparency = (v: unknown): number =>
  clampNumber(v, PANEL_TRANSPARENCY_MIN, PANEL_TRANSPARENCY_MAX, PANEL_TRANSPARENCY_DEFAULT);
export const parsePanelBlur = (v: unknown): number => clampNumber(v, PANEL_BLUR_MIN, PANEL_BLUR_MAX, PANEL_BLUR_DEFAULT);
export const parseTransparentChat = (v: unknown): boolean => v === true || v === "true" || v === "1";

/* ---------- backdrop tone ---------- */

/** The lightest and the darkest colour a backdrop shows (after its dim layer). */
export interface ToneExtremes {
  light: string;
  dark: string;
}

/**
 * `peak`: per-pixel extremes (what an unblurred layer sits on: tinted panels, the scrim, bubbles).
 * `soft`: extremes of ~32 px cell averages (what a blurred glass layer actually sees).
 */
export interface BackdropTone {
  peak: ToneExtremes;
  soft: ToneExtremes;
  /**
   * Local texture: the brightest and darkest pixel inside one ~32 px cell, for the cell at the 90th
   * percentile of within-cell contrast (dots, dither, grain).  Absent = flat.
   */
  detail?: ToneExtremes;
}

/** Unknown backdrop (an image that cannot be sampled, or before the first sample): pure white / black. */
export const WORST_TONE: BackdropTone = {
  peak: { light: "#ffffff", dark: "#000000" },
  soft: { light: "#ffffff", dark: "#000000" }
};

/** The backdrop colour that hurts this theme's text most: the lightest for dark themes, the darkest for light ones. */
export function worstBackdrop(ext: ToneExtremes, d: Pick<DerivedTheme, "light">): string {
  return d.light ? ext.dark : ext.light;
}

/* ---------- the plan ---------- */

/** Primary text reaches AAA on every translucent shell layer, so muted text keeps room for 4.5:1. */
export const TEXT_TARGET = 7;

/** Comfortable alpha range of each material (the floor may push the minimum up, never down). */
const MATERIAL: Record<"glass" | "tinted", { min: number; max: number }> = {
  glass: { min: 0.3, max: 0.9 },
  tinted: { min: 0.55, max: 0.95 }
};
/** Backdrops whose local texture reaches this contrast (light vs dark pixel in one cell) count as busy. */
export const BUSY_DETAIL = 1.4;
/** A busy backdrop always gets at least this much scrim under the conversation. */
export const BUSY_SCRIM_MIN = 0.45;
/** The line style lets the backdrop through at this alpha (unchanged from 198-A). */
const LINE_ALPHA = 0.72;
/** Cards inside a translucent panel (settings groups, panel cards, bubbles): alpha of their own fill. */
export const CARD_ALPHA = { surface: 0.52, raised: 0.55 };

export interface PlatformSurfaceSupport {
  /** `CSS.supports("backdrop-filter", "blur(1px)")`. */
  backdropFilter: boolean;
  /** `prefers-reduced-transparency: reduce`. */
  reducedTransparency: boolean;
}

export const FULL_SUPPORT: PlatformSurfaceSupport = { backdropFilter: true, reducedTransparency: false };

/** Style actually painted: reduced transparency forces solid; glass without backdrop-filter becomes tinted. */
export function effectiveUiStyle(style: UiStyle, env: PlatformSurfaceSupport = FULL_SUPPORT): UiStyle {
  if (env.reducedTransparency) return "solid";
  if (style === "glass" && !env.backdropFilter) return "tinted";
  return style;
}

/* ---------- platform (guarded: safe to call in node) ---------- */

const REDUCED_TRANSPARENCY = "(prefers-reduced-transparency: reduce)";

let reducedTransparencyOverridden = false;

/**
 * The user explicitly chose their panel style over the OS "reduce transparency" preference
 * (Windows: Settings > Personalisation > Colours > Transparency effects off).  Set by the
 * appearance hook before the shell lays out, so `paintedUiStyle` and `applyTheme` agree.
 */
export function setReducedTransparencyOverride(on: boolean): void {
  reducedTransparencyOverridden = on;
}

/** The OS asks for reduced transparency (regardless of the user's override). */
export function osPrefersReducedTransparency(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(REDUCED_TRANSPARENCY).matches;
}

/** What this WebView can paint: backdrop-filter support and the OS "reduce transparency" preference. */
export function surfaceSupport(): PlatformSurfaceSupport {
  if (typeof window === "undefined") return { backdropFilter: true, reducedTransparency: false };
  const supports = typeof CSS !== "undefined" && typeof CSS.supports === "function";
  const backdropFilter =
    !supports || CSS.supports("backdrop-filter", "blur(1px)") || CSS.supports("-webkit-backdrop-filter", "blur(1px)");
  const reducedTransparency = !reducedTransparencyOverridden && osPrefersReducedTransparency();
  return { backdropFilter, reducedTransparency };
}

/** Re-run `onChange` when the OS transparency preference changes. */
export function subscribeSurfaceSupport(onChange: () => void): () => void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return () => {};
  const mq = window.matchMedia(REDUCED_TRANSPARENCY);
  mq.addEventListener("change", onChange);
  return () => mq.removeEventListener("change", onChange);
}

/** The style that is actually painted for a saved style on this platform. */
export function paintedUiStyle(style: UiStyle): UiStyle {
  return effectiveUiStyle(style, surfaceSupport());
}

/** Panels float (padding, radius, shadow) in the glass and tinted styles or over a backdrop. */
export function isFloatingStyle(style: UiStyle): boolean {
  return style === "glass" || style === "tinted";
}

/** Smallest alpha (0..1, 0.01 steps) at which `text` reaches `target` on `tint` laid over `worst`. */
export function legibleAlpha(text: string, tint: string, worst: string, target: number = TEXT_TARGET): number {
  for (let i = 0; i <= 100; i++) {
    const a = i / 100;
    if (contrastRatio(text, mix(worst, tint, a)) >= target) return a;
  }
  return 1;
}

export interface SurfaceLayer {
  /** Name used in contrast reports. */
  name: string;
  /** The opaque colour text actually sits on (layer composited over the worst backdrop). */
  color: string;
  /** Primary text must reach this on the layer (TEXT_TARGET on shell layers, 4.5 elsewhere). */
  textMin: number;
}

export interface SurfacePlan {
  /** Effective style (after platform fallbacks). */
  style: UiStyle;
  /** Shell panels let the backdrop through. */
  translucent: boolean;
  blurred: boolean;
  /** Shell panel tint and alpha (1 = opaque). */
  tint: string;
  shellAlpha: number;
  /** The center pane has no background of its own. */
  clearChat: boolean;
  /** Colour and alpha of the theme-tinted scrim under the reading column of a clear chat area. */
  scrimTint: string;
  scrimAlpha: number;
  /** Floating surfaces (modals, popovers, menus, toasts). */
  floatTint: string;
  floatAlpha: number;
  /** Composer over a clear chat area (blurred in the glass style). */
  composerAlpha: number;
  blur: number;
  saturate: number;
  /** Every opaque colour text may sit on, besides the theme's solid surfaces. */
  layers: SurfaceLayer[];
}

const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const round2 = (n: number) => Math.round(n * 100) / 100;

/** Plan the surfaces for a theme, an effective style and the user's options. */
export function planSurfaces(
  d: DerivedTheme,
  style: UiStyle,
  backdropActive: boolean,
  opts: SurfaceOptions
): SurfacePlan {
  const tone = opts.tone ?? WORST_TONE;
  const peak = worstBackdrop(tone.peak, d);
  const soft = worstBackdrop(tone.soft, d);
  const t = parsePanelTransparency(opts.transparency) / 100;
  const blur = parsePanelBlur(opts.blur);
  // Dark glass is the theme's own background; light glass leans to white so Clay does not turn beige-grey.
  const tint = d.light ? mix(d.bg, "#ffffff", 0.45) : d.bg;
  const blurred = style === "glass";
  const translucent = style === "glass" || style === "tinted" || (style === "line" && backdropActive);
  const behindShell = blurred ? soft : peak;

  let shellAlpha = 1;
  if (style === "glass" || style === "tinted") {
    const m = MATERIAL[style];
    const floor = Math.max(m.min, legibleAlpha(d.text, tint, behindShell));
    shellAlpha = Math.max(floor, lerp(m.max, floor, t));
  } else if (style === "line" && backdropActive) {
    shellAlpha = Math.max(LINE_ALPHA, legibleAlpha(d.text, d.bg, peak));
  }
  shellAlpha = round2(shellAlpha);
  // Solid and line panels keep the theme background (unchanged look); only glass / tinted lean airy.
  const shellTint = style === "glass" || style === "tinted" ? tint : d.bg;

  const clearChat = opts.transparentChat;
  // The scrim is as light as legibility allows: over a dark backdrop it disappears.  Light themes
  // use the airy panel tint so the scrim does not grey a colourful backdrop.
  const scrimTint = d.light ? tint : d.bg;
  // Fine, high-contrast detail (dither, dots, grain) is noisy behind text even when the contrast
  // ratio passes; the gap between the per-pixel and the blurred extreme measures it.
  const busy = tone.detail !== undefined && contrastRatio(tone.detail.light, tone.detail.dark) >= BUSY_DETAIL;
  const scrimAlpha = clearChat ? round2(Math.max(busy ? BUSY_SCRIM_MIN : 0, legibleAlpha(d.text, scrimTint, peak))) : 0;

  const floatTint = d.light ? mix(d.raised, "#ffffff", 0.3) : d.surface;
  let floatAlpha = 1;
  if (style === "glass") floatAlpha = Math.max(0.78, shellAlpha, legibleAlpha(d.text, floatTint, soft));
  else if (style === "tinted") floatAlpha = Math.max(0.94, shellAlpha);
  floatAlpha = round2(floatAlpha);

  // The composer keeps a readable surface of its own when the chat area is clear.
  const composerAlpha = clearChat
    ? round2(Math.max(blurred ? 0.62 : 0.84, legibleAlpha(d.text, d.raised, blurred ? soft : peak)))
    : 1;

  const layers: SurfaceLayer[] = [];
  if (translucent) {
    const shell = mix(behindShell, shellTint, shellAlpha);
    layers.push({ name: `${style} panel`, color: shell, textMin: TEXT_TARGET });
    layers.push({ name: `${style} panel card`, color: mix(shell, d.surface, CARD_ALPHA.surface), textMin: 4.5 });
    layers.push({ name: `${style} panel raised card`, color: mix(shell, d.raised, CARD_ALPHA.raised), textMin: 4.5 });
  }
  if (clearChat) {
    const scrim = mix(peak, scrimTint, scrimAlpha);
    layers.push({ name: "chat scrim", color: scrim, textMin: TEXT_TARGET });
    layers.push({ name: "bubble on scrim", color: mix(scrim, d.raised, CARD_ALPHA.raised), textMin: 4.5 });
    layers.push({ name: "composer", color: mix(blurred ? soft : peak, d.raised, composerAlpha), textMin: 4.5 });
  }
  if (floatAlpha < 1) {
    layers.push({ name: `${style} floating surface`, color: mix(blurred ? soft : peak, floatTint, floatAlpha), textMin: 4.5 });
  }

  return {
    style,
    translucent,
    blurred,
    tint: shellTint,
    shellAlpha,
    clearChat,
    scrimTint,
    scrimAlpha,
    floatTint,
    floatAlpha,
    composerAlpha,
    blur,
    saturate: d.light ? 1.9 : 1.7,
    layers
  };
}

/* ---------- text colours on translucent layers ---------- */

/** `mix(text, bg, t)` with `t` reduced until it reaches `min` on every colour in `against`. */
function fadeOn(text: string, bg: string, t: number, against: readonly string[], min: number): string {
  for (let w = t; w > 0.0001; w -= 0.02) {
    const c = mix(text, bg, w);
    if (against.every((a) => contrastRatio(c, a) >= min)) return c;
  }
  return text;
}

export interface SurfaceText {
  secondary: string;
  muted: string;
  faint: string;
  lineStrong: string;
  success: string;
  warning: string;
  danger: string;
  accentText: string;
}

/** Text colours re-derived so they stay readable on every layer of the plan (and the solid surfaces). */
export function surfaceText(d: DerivedTheme, accent: string, plan: SurfacePlan): SurfaceText {
  const solid = [d.bg, d.surface, d.raised, d.hover];
  const against = [...solid, ...plan.layers.map((l) => l.color)];
  const muted = fadeOn(d.text, d.bg, 0.38, against, 4.5);
  const push = (c: string) => pushContrast(c, d.text, against, 4.5);
  return {
    secondary: fadeOn(d.text, d.bg, 0.19, against, 4.5),
    muted,
    faint: fadeOn(d.text, d.bg, 0.62, against, 3),
    lineStrong: fadeOn(d.text, d.bg, 0.5, plan.layers.map((l) => l.color).concat([d.bg, d.surface, d.raised]), 3),
    success: push(d.success),
    warning: push(d.warning),
    danger: push(d.danger),
    accentText: pushContrast(accent, d.text, against, 4.5)
  };
}

/* ---------- tokens ---------- */

const alpha = (color: string, a: number) =>
  a >= 1 ? color : `color-mix(in srgb, ${color} ${Math.round(a * 100)}%, transparent)`;

/**
 * Fine grain laid over glass: breaks up 8-bit banding in the blurred gradients and gives the
 * material a tactile surface.  A static SVG, rasterised once by the browser.
 */
function grain(light: boolean): string {
  const o = light ? 0.05 : 0.04;
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='160' height='160'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='.85' numOctaves='2' stitchTiles='stitch'/><feColorMatrix values='0 0 0 0 .5 0 0 0 0 .5 0 0 0 0 .5 0 0 0 ${o} 0'/></filter><rect width='100%' height='100%' filter='url(%23n)'/></svg>`;
  return `url("data:image/svg+xml,${svg.replace(/</g, "%3C").replace(/>/g, "%3E").replace(/"/g, "'")}")`;
}

/** Shell / floating surface tokens for a plan (merged over `buildTokens`). */
export function surfaceTokens(d: DerivedTheme, accent: string, plan: SurfacePlan): Record<string, string> {
  const ink = d.light ? "#28283c" : "#000000";
  const glassy = plan.style === "glass" || plan.style === "tinted";
  const filter = plan.blurred ? `blur(${plan.blur}px) saturate(${plan.saturate})` : "none";
  // Hairline: white-ish on dark glass, a faint ink ring under a white edge on light glass.
  const border = glassy
    ? d.light
      ? alpha(ink, 0.11)
      : alpha("#ffffff", 0.09)
    : alpha(d.text, 0.08);
  const highlight = glassy
    ? `inset 0 1px 0 ${alpha("#ffffff", d.light ? 0.75 : 0.1)}, inset 0 0 0 1px ${alpha("#ffffff", d.light ? 0.22 : 0.025)}`
    : `inset 0 1px 0 ${alpha(d.text, d.light ? 0 : 0.04)}`;
  const shadow = d.light
    ? `0 1px 1px ${alpha(ink, 0.05)}, 0 6px 16px -6px ${alpha(ink, 0.1)}, 0 28px 56px -28px ${alpha(ink, 0.22)}`
    : `0 1px 1px ${alpha(ink, 0.3)}, 0 8px 24px -8px ${alpha(ink, 0.35)}, 0 32px 64px -32px ${alpha(ink, 0.55)}`;
  const shellFill = alpha(plan.tint, plan.shellAlpha);
  const tokens: Record<string, string> = {
    "--shell-fill": shellFill,
    "--shell-filter": filter,
    "--shell-border": border,
    "--shell-highlight": highlight,
    "--shell-shadow": shadow,
    "--shell-grain": plan.blurred ? grain(d.light) : "none",
    "--center-fill": plan.clearChat ? "transparent" : shellFill,
    "--center-filter": plan.clearChat ? "none" : filter,
    "--center-border": plan.clearChat ? "transparent" : border,
    "--center-highlight": plan.clearChat ? "inset 0 0 0 transparent" : highlight,
    "--center-shadow": plan.clearChat ? "0 0 0 transparent" : shadow,
    "--center-scrim": plan.clearChat ? alpha(plan.scrimTint, plan.scrimAlpha) : "transparent",
    "--composer-fill": plan.clearChat ? alpha(d.raised, plan.composerAlpha) : "var(--color-surface-elevated)",
    "--composer-filter": plan.clearChat && plan.blurred ? `blur(${Math.round(plan.blur * 0.75)}px) saturate(${plan.saturate})` : "none",
    "--float-fill": alpha(plan.floatTint, plan.floatAlpha),
    "--float-filter": plan.blurred ? `blur(${Math.round(plan.blur * 1.2)}px) saturate(${plan.saturate})` : "none",
    "--float-border": glassy ? border : alpha(d.text, 0.14),
    "--float-highlight": glassy
      ? `inset 0 1px 0 ${alpha("#ffffff", d.light ? 0.8 : 0.12)}`
      : `inset 0 1px 0 ${alpha(d.text, 0.04)}`,
    "--bubble-glass-fill": alpha(d.raised, plan.clearChat ? Math.max(0.6, plan.composerAlpha - 0.1) : 0.55),
    "--radius-shell": "calc(var(--radius-card) * 1.35)"
  };
  if (plan.layers.length) {
    const s = surfaceText(d, accent, plan);
    Object.assign(tokens, {
      "--color-text-secondary": s.secondary,
      "--color-text-muted": s.muted,
      "--color-text-faint": s.faint,
      "--color-border-strong": s.lineStrong,
      "--color-focus": s.lineStrong,
      "--color-success": s.success,
      "--color-warning": s.warning,
      "--color-danger": s.danger,
      "--color-accent-text": s.accentText,
      "--color-accent-deep": s.accentText,
      "--color-code-keyword": s.accentText,
      "--color-code-string": s.success,
      "--color-code-number": s.warning,
      "--color-code-comment": s.muted
    });
  }
  return tokens;
}

/**
 * All tokens for a theme: `buildTokens` (colours, shadows) plus the surface system.  `style` is
 * the *effective* style (see `effectiveUiStyle`).
 */
export function buildThemeTokens(
  theme: ThemeDef,
  accentColor: string,
  style: UiStyle = "solid",
  backdropActive = false,
  surface: SurfaceOptions = DEFAULT_SURFACE
): Record<string, string> {
  const base = buildTokens(theme, accentColor, style, backdropActive);
  const d = deriveTheme(theme);
  const plan = planSurfaces(d, style, backdropActive, surface);
  return { ...base, ...surfaceTokens(d, base["--color-accent"], plan) };
}
