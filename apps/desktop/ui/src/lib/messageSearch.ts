import { foldText } from "./searchText.ts";

/** How far back a search looks. `any` is the default. */
export type SearchWhen = "any" | "day" | "week" | "month";

/** The small chips next to the search box. */
export interface SearchFilters {
  botId: string | null;
  when: SearchWhen;
  hasFile: boolean;
  hasLink: boolean;
}

export const NO_FILTERS: SearchFilters = { botId: null, when: "any", hasFile: false, hasLink: false };

const DAY_MS = 86_400_000;
const WINDOW_MS: Record<Exclude<SearchWhen, "any">, number> = {
  day: DAY_MS,
  week: 7 * DAY_MS,
  month: 30 * DAY_MS
};

/** True when at least one chip narrows the search. */
export function filtersActive(f: SearchFilters): boolean {
  return f.botId !== null || f.when !== "any" || f.hasFile || f.hasLink;
}

/** Query parameters of `GET /v1/chats/search` for the chosen chips (only the ones that narrow). */
export function filterParams(f: SearchFilters, now: number): Record<string, string | number | boolean> {
  const out: Record<string, string | number | boolean> = {};
  if (f.botId) out.bot_id = f.botId;
  if (f.when !== "any") out.from_ms = now - WINDOW_MS[f.when];
  if (f.hasFile) out.has_file = true;
  if (f.hasLink) out.has_link = true;
  return out;
}

const RECENT_KEY = "praxi.recentSearches";
/** Few enough to scan at a glance. */
export const RECENT_MAX = 5;

/** Newest first, no duplicates (ignoring case, accents and the Turkish dotless i), at most {@link RECENT_MAX}. */
export function pushRecent(list: readonly string[], term: string): string[] {
  const clean = term.trim().replace(/\s+/g, " ");
  if (!clean) return [...list];
  const key = foldText(clean);
  return [clean, ...list.filter((x) => foldText(x) !== key)].slice(0, RECENT_MAX);
}

type Store = Pick<Storage, "getItem" | "setItem" | "removeItem">;

function store(): Store | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** Recent search terms kept on this device only (never sent anywhere). */
export function loadRecent(s: Store | null = store()): string[] {
  try {
    const raw = s?.getItem(RECENT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed)
      ? parsed.filter((x): x is string => typeof x === "string" && x.length > 0 && x.length <= 200).slice(0, RECENT_MAX)
      : [];
  } catch {
    return [];
  }
}

export function saveRecent(list: readonly string[], s: Store | null = store()): void {
  try {
    if (list.length === 0) s?.removeItem(RECENT_KEY);
    else s?.setItem(RECENT_KEY, JSON.stringify(list.slice(0, RECENT_MAX)));
  } catch {
    // Storage full or blocked: recent searches are a convenience only.
  }
}

/** Next row for the arrow keys; wraps around, `-1` when there is nothing to select. */
export function stepCursor(current: number, count: number, dir: 1 | -1): number {
  if (count <= 0) return -1;
  if (current < 0 || current >= count) return dir === 1 ? 0 : count - 1;
  return (current + dir + count) % count;
}

/** A short day label: "14:05" today, otherwise the date, with the year only when it is not this year. */
export function resultDate(ms: number, now: number, locale: string): string {
  const d = new Date(ms);
  const n = new Date(now);
  if (d.toDateString() === n.toDateString()) {
    return d.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });
  }
  return d.toLocaleDateString(
    locale,
    d.getFullYear() === n.getFullYear()
      ? { day: "numeric", month: "short" }
      : { day: "numeric", month: "short", year: "numeric" }
  );
}
