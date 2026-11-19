import { parseVoiceName, parseVoiceRate } from "./voicePrefs.ts";

/**
 * Per-bot read-aloud overrides (ADR 0021: voice and speed live with the bot). Kept in this device's
 * local storage like the other spoken-reply preferences; a bot without an entry uses the global ones.
 */
export interface BotVoice {
  voiceName: string;
  rate: number;
}

const KEY = "praxi.botVoice.v1";

function readAll(): Record<string, unknown> {
  try {
    const raw = window.localStorage.getItem(KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

export function getBotVoice(botId: string | null | undefined): BotVoice | null {
  const v = botId ? readAll()[botId] : undefined;
  if (!v || typeof v !== "object") {
    return null;
  }
  const o = v as Record<string, unknown>;
  return { voiceName: parseVoiceName(o.voiceName), rate: parseVoiceRate(o.rate) };
}

/** Save the override; `null` removes it (back to the global voice). */
export function setBotVoice(botId: string, voice: BotVoice | null): void {
  try {
    const all = readAll();
    if (voice) {
      all[botId] = { voiceName: parseVoiceName(voice.voiceName), rate: parseVoiceRate(voice.rate) };
    } else {
      delete all[botId];
    }
    window.localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    // Storage unavailable: the global voice keeps being used.
  }
}

/** The voice to read a bot's reply with: the bot's own override, else the global preference. */
export function effectiveVoice(botId: string | null | undefined, global: BotVoice): BotVoice {
  return getBotVoice(botId) ?? global;
}
