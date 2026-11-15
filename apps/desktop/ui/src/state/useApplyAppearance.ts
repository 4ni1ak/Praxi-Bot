import { useEffect, useState, useSyncExternalStore } from "react";
import {
  accentFor,
  applyTheme,
  deriveTheme,
  resolveTheme,
  setReducedTransparencyOverride,
  subscribeSurfaceSupport,
  subscribeSystemTheme,
  type BackdropTone,
  type Theme
} from "ui";
import { backdropIsActive, CUSTOM_BACKDROP, NO_BACKDROP } from "../lib/backdropCatalog";
import { dimTone, getSceneTone, plainTone, sampleImageTone, subscribeSceneTone } from "../lib/backdropTone";
import { loadSettingsSync, type AppSettings } from "../store/settingsStore";

export interface AppliedAppearance {
  theme: Theme;
  /** The resolved accent colour. */
  accent: string;
}

type AppearanceInput = Pick<AppSettings, "themeMode" | "customTheme" | "accentColor">;

/** The window's own backdrop / style axes only apply to the main window. */
type FullAppearanceInput = AppearanceInput &
  Pick<
    AppSettings,
    | "uiStyle"
    | "corners"
    | "bubble"
    | "ownBubble"
    | "backdrop"
    | "backgroundUrl"
    | "backgroundBlur"
    | "backgroundDim"
    | "backdropDim"
    | "panelTransparency"
    | "panelBlur"
    | "transparentChat"
    | "transparencyOverride"
  >;

/** Write the appearance onto <html> (no React): also used before the first render. */
export function paintAppearance(
  settings: FullAppearanceInput | AppearanceInput,
  theme: Theme,
  accent: string,
  full: boolean,
  tone?: BackdropTone
): void {
  const f = settings as FullAppearanceInput;
  if (full) setReducedTransparencyOverride(f.transparencyOverride === true);
  applyTheme(theme, {
    accentColor: accent,
    ...(full
      ? {
          style: f.uiStyle,
          corners: f.corners,
          bubble: f.bubble,
          ownBubble: f.ownBubble,
          backdropActive: backdropIsActive(f.backdrop, f.backgroundUrl),
          background: { url: f.backgroundUrl, blur: f.backgroundBlur, dim: f.backgroundDim },
          surface: {
            transparency: f.panelTransparency,
            blur: f.panelBlur,
            transparentChat: f.transparentChat,
            tone
          }
        }
      : {})
  });
}

/** Apply the saved settings synchronously (localStorage mirror) so the first paint has the right colours. */
export function paintSavedAppearanceEarly(overlay: boolean): void {
  document.documentElement.dataset.booting = "1";
  const saved = loadSettingsSync();
  if (!saved) return;
  const theme = resolveTheme(saved.themeMode, saved.customTheme);
  paintAppearance(saved, theme, accentFor(saved.accentColor, theme, undefined), !overlay);
}

const noopSubscribe = () => () => {};

/**
 * What the backdrop behind the panels looks like (after its dim layers), or `undefined` while it
 * is unknown - the surface model then assumes a pure white / black backdrop.
 */
function useBackdropTone(full: FullAppearanceInput | null, theme: Theme): BackdropTone | undefined {
  const scene = full && full.backdrop !== NO_BACKDROP && full.backdrop !== CUSTOM_BACKDROP ? full.backdrop : null;
  const imageUrl = full && !scene ? full.backgroundUrl.trim() : "";
  const snapshot = () => (scene ? getSceneTone(scene) : null);
  const raw = useSyncExternalStore(scene ? subscribeSceneTone : noopSubscribe, snapshot, snapshot);
  const [image, setImage] = useState<{ url: string; tone: BackdropTone | null } | null>(null);
  useEffect(() => {
    if (!imageUrl) return undefined;
    let live = true;
    void sampleImageTone(imageUrl, window.innerWidth, window.innerHeight).then((tone) => {
      if (live) setImage({ url: imageUrl, tone });
    });
    return () => {
      live = false;
    };
  }, [imageUrl]);
  if (!full) return undefined;
  const d = deriveTheme(theme);
  const imageDim = Number(full.backgroundDim) || 0;
  if (scene) return raw ? dimTone(dimTone(raw, d.bg, full.backdropDim / 100), d.bg, imageDim) : undefined;
  if (imageUrl) {
    const tone = image?.url === imageUrl ? image.tone : null;
    return tone ? dimTone(tone, d.bg, imageDim) : undefined;
  }
  return plainTone(d.bg, d.bg2);
}

/**
 * Paint the saved appearance onto <html> and re-paint when the theme, the
 * accent (which may follow the active bot) or the OS colour scheme changes.
 * Returns the theme and accent in effect.
 */
export function useApplyAppearance(
  settings: FullAppearanceInput,
  botColor: string | undefined
): AppliedAppearance;
export function useApplyAppearance(
  settings: AppearanceInput,
  botColor: string | undefined,
  overlay: true
): AppliedAppearance;
export function useApplyAppearance(
  settings: FullAppearanceInput | AppearanceInput,
  botColor: string | undefined,
  overlay?: true
): AppliedAppearance {
  // Re-resolve `system` when the OS switches between light and dark.
  const [systemTick, setSystemTick] = useState(0);
  useEffect(() => {
    if (settings.themeMode !== "system") return undefined;
    return subscribeSystemTheme(() => setSystemTick((n) => n + 1));
  }, [settings.themeMode]);
  // Re-paint when the OS "reduce transparency" preference flips.
  useEffect(() => subscribeSurfaceSupport(() => setSystemTick((n) => n + 1)), []);

  const theme = resolveTheme(settings.themeMode, settings.customTheme);
  const accent = accentFor(settings.accentColor, theme, botColor);
  const full = overlay ? null : (settings as FullAppearanceInput);
  // Before the shell computes its geometry (isFloatShell) in this same render.
  if (full) setReducedTransparencyOverride(full.transparencyOverride === true);
  const tone = useBackdropTone(full, theme);
  const dep = [
    systemTick,
    tone ? JSON.stringify(tone) : "",
    full?.backdropDim,
    full?.panelTransparency,
    full?.panelBlur,
    full?.transparentChat,
    full?.transparencyOverride,
    theme.id,
    theme.bg,
    theme.text,
    settings.accentColor,
    botColor,
    full?.uiStyle,
    full?.corners,
    full?.bubble,
    full?.ownBubble,
    full?.backdrop,
    full?.backgroundUrl,
    full?.backgroundBlur,
    full?.backgroundDim
  ];
  useEffect(() => {
    paintAppearance(settings, theme, accent, full !== null, tone);
    // First paint is done: colour transitions may run from now on.
    const id = requestAnimationFrame(() => requestAnimationFrame(() => delete document.documentElement.dataset.booting));
    return () => cancelAnimationFrame(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `dep` lists every input
  }, dep);
  return { theme, accent };
}

