/**
 * "What's new" after an update: notes come from the verified update saved locally, or bundled
 * `whats-new.json` entries. The last version seen lives in local settings; no feed is fetched at startup.
 */
export type NotesLang = "en" | "tr" | "de";

export interface WhatsNewNotes {
  title: string;
  items: string[];
  text?: string;
}

/** Feed content stays inert text, and belongs only to the version actually installed. */
export function installedReleaseNotes(
  notes: { version: string; notes: string } | null,
  version: string,
  title: string
): WhatsNewNotes | null {
  return notes?.version === version && notes.notes.trim()
    ? { title, items: [], text: notes.notes }
    : null;
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
