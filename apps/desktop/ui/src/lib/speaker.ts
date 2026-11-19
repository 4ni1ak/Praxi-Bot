import { pickVoice } from "./voicePrefs";

/**
 * Reads text aloud with the operating system's speech synthesis (`window.speechSynthesis`):
 * nothing leaves the machine unless the user picks an online voice, no key or account is needed.
 * One utterance at a time; starting another replaces the current one.
 */

type Listener = () => void;

let speakingId: string | null = null;
const listeners = new Set<Listener>();

function emit(): void {
  listeners.forEach((l) => l());
}

export function speechSupported(): boolean {
  return typeof window !== "undefined" && "speechSynthesis" in window && typeof SpeechSynthesisUtterance !== "undefined";
}

export function subscribeSpeaking(l: Listener): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function currentSpeakingId(): string | null {
  return speakingId;
}

export function stopSpeaking(): void {
  if (speechSupported()) {
    window.speechSynthesis.cancel();
  }
  speakingId = null;
  emit();
}

/** Voices the OS offers (may be empty until the engine has loaded them). */
export function listVoices(): SpeechSynthesisVoice[] {
  return speechSupported() ? window.speechSynthesis.getVoices() : [];
}

export function speak(id: string, text: string, opts: { voiceName: string; rate: number; lang: string }): void {
  if (!speechSupported() || !text.trim()) {
    return;
  }
  window.speechSynthesis.cancel();
  const u = new SpeechSynthesisUtterance(text);
  const voice = pickVoice(listVoices(), opts.voiceName, opts.lang);
  if (voice) {
    u.voice = voice;
    u.lang = voice.lang;
  } else {
    u.lang = opts.lang;
  }
  u.rate = opts.rate;
  const done = () => {
    if (speakingId === id) {
      speakingId = null;
      emit();
    }
  };
  u.onend = done;
  u.onerror = done;
  speakingId = id;
  emit();
  window.speechSynthesis.speak(u);
}
