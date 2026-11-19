/** Spoken-reply preferences (Settings > General). The engine is the OS speech synthesis. */

export const VOICE_RATE_MIN = 0.5;
export const VOICE_RATE_MAX = 2;
export const DEFAULT_VOICE_RATE = 1;

/** Narrow a stored speaking speed to the supported range (defaults to 1). */
export function parseVoiceRate(v: unknown): number {
  if (typeof v !== "number" || !Number.isFinite(v)) {
    return DEFAULT_VOICE_RATE;
  }
  return Math.min(VOICE_RATE_MAX, Math.max(VOICE_RATE_MIN, Math.round(v * 20) / 20));
}

/** A stored voice name, or "" (automatic: a voice matching the UI language). */
export function parseVoiceName(v: unknown): string {
  return typeof v === "string" ? v.slice(0, 200) : "";
}

export interface VoiceLike {
  name: string;
  lang: string;
  localService?: boolean;
}

/**
 * The voice to speak with: the chosen one if still installed, else the first local voice whose
 * language matches the UI language, else any voice of that language, else null (engine default).
 */
export function pickVoice<T extends VoiceLike>(voices: readonly T[], name: string, uiLang: string): T | null {
  const chosen = name ? voices.find((v) => v.name === name) : undefined;
  if (chosen) {
    return chosen;
  }
  const base = uiLang.toLowerCase().split("-")[0] ?? "";
  const same = voices.filter((v) => v.lang.toLowerCase().split(/[-_]/)[0] === base);
  return same.find((v) => v.localService) ?? same[0] ?? null;
}
