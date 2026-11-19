/**
 * When this computer last synced its setup to a server ("Kurulumumu eşitle"). The daemon does not
 * keep a time for it, so the app remembers the moment a copy finished, per server, in this
 * browser profile only (a convenience for the status menu, never a source of truth).
 */
const KEY = (hostId: string) => `praxi.lastSync.${hostId}`;

export function readLastSync(hostId: string): number | null {
  try {
    const v = Number(window.localStorage.getItem(KEY(hostId)));
    return Number.isFinite(v) && v > 0 ? v : null;
  } catch {
    return null;
  }
}

export function writeLastSync(hostId: string, ms: number): void {
  try {
    window.localStorage.setItem(KEY(hostId), String(ms));
  } catch {
    // private mode or blocked storage: the menu simply shows no time
  }
}

/** "today" or a short date, plus the time: `{ day: null, time: "01:12" }` for today. */
export function lastSyncParts(ms: number, now: number, lang: string): { day: string | null; time: string } {
  const d = new Date(ms);
  const time = new Intl.DateTimeFormat(lang, { hour: "2-digit", minute: "2-digit", hourCycle: lang.startsWith("tr") ? "h23" : undefined }).format(d);
  const sameDay = d.toDateString() === new Date(now).toDateString();
  return {
    day: sameDay ? null : new Intl.DateTimeFormat(lang, { day: "numeric", month: "short" }).format(d),
    time
  };
}
