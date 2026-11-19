/**
 * "What's new" after an update: the notes are bundled (`whats-new.json`, one entry per app version in
 * en/tr/de) and the last version the user saw lives in the local settings. Nothing is fetched.
 */
export type NotesLang = "en" | "tr" | "de";

export interface WhatsNewNotes {
  title: string;
  items: string[];
}

export type WhatsNewFile = Record<string, Partial<Record<NotesLang, WhatsNewNotes>>>;

/** Notes of `version` in the UI language (English when that language has none); null when the version has none. */
export function notesFor(file: WhatsNewFile, version: string, language: string): WhatsNewNotes | null {
  const entry = file[version];
  if (!entry) return null;
  const lang = language.slice(0, 2).toLowerCase() as NotesLang;
  return entry[lang] ?? entry.en ?? null;
}

/**
 * What to do at start-up: `show` the card, `record` the version silently (fresh install, or a version
 * without notes), or `none`. A first run that never finished onboarding is a fresh install, not an update.
 */
export function whatsNewAction(args: {
  version: string;
  lastSeen: string;
  onboardingCompleted: boolean;
  hasNotes: boolean;
}): "show" | "record" | "none" {
  if (!args.version || args.lastSeen === args.version) return "none";
  if (args.lastSeen === "" && !args.onboardingCompleted) return "record";
  return args.hasNotes ? "show" : "record";
}
