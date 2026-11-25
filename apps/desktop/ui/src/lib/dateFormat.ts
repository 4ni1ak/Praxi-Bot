/**
 * One way to write a date and time on the pages (Schedules, My bots, Needs you, Usage, ...):
 * "Nov 25, 5:09 PM", the year only when it is not the current year, 24 h for tr and de.
 * Pure, so the wording is tested without a browser. Times are local unless a zone is given.
 */

/** The UI language drives the wording, never the OS locale; tr and de are always 24 h. */
function locale(lang: string): string {
  const l = lang.toLowerCase();
  return l.startsWith("tr") ? "tr-TR" : l.startsWith("de") ? "de-DE" : lang;
}

function hourCycle(lang: string): { hourCycle?: "h23" } {
  const l = lang.toLowerCase();
  return l.startsWith("tr") || l.startsWith("de") ? { hourCycle: "h23" } : {};
}

export interface WhenOptions {
  /** IANA zone to show the wall clock of (a schedule's own zone); default is the viewer's. */
  zone?: string;
  /** Add the zone name ("GMT+3") because it differs from the viewer's. */
  zoneName?: boolean;
  /** "Now" for the current-year check (tests); default is the real clock. */
  now?: number;
}

/** "Nov 25, 5:09 PM" or, in another year, "Nov 25, 2025, 5:09 PM". */
export function formatWhen(ts: number, lang: string, opts: WhenOptions = {}): string {
  if (!Number.isFinite(ts)) return "";
  const zone = opts.zone ? { timeZone: opts.zone } : {};
  const year = (t: number) => Number(new Intl.DateTimeFormat("en", { year: "numeric", ...zone }).format(t));
  const sameYear = year(ts) === year(opts.now ?? Date.now());
  return new Intl.DateTimeFormat(locale(lang), {
    ...(sameYear ? {} : { year: "numeric" as const }),
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    ...hourCycle(lang),
    ...zone,
    ...(opts.zoneName ? { timeZoneName: "short" as const } : {})
  }).format(ts);
}

/** "5:09 PM" (24 h in tr and de). */
export function formatTimeOfDay(ts: number, lang: string): string {
  if (!Number.isFinite(ts)) return "";
  return new Intl.DateTimeFormat(locale(lang), {
    hour: "numeric",
    minute: "2-digit",
    ...hourCycle(lang)
  }).format(ts);
}

/** "Nov 25" or, in another year, "Nov 25, 2025". */
export function formatDay(ts: number, lang: string, now: number = Date.now()): string {
  if (!Number.isFinite(ts)) return "";
  const sameYear = new Date(ts).getFullYear() === new Date(now).getFullYear();
  return new Intl.DateTimeFormat(locale(lang), {
    ...(sameYear ? {} : { year: "numeric" as const }),
    month: "short",
    day: "numeric"
  }).format(ts);
}
