/** Date + time for the schedule builder's run preview, e.g. "Mon, Nov 23, 2026, 9:00 AM".
 *  Do not combine `dateStyle`/`timeStyle` with `weekday`: browsers throw a RangeError and the whole
 *  builder goes blank. Explicit fields are always safe. */
export function formatRunPreview(ms: number, locale: string, timeZone?: string): string {
  return new Date(ms).toLocaleString(locale, {
    weekday: "short",
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    ...(timeZone ? { timeZone } : {})
  });
}

function validZone(zone: string): boolean {
  try {
    new Intl.DateTimeFormat("en", { timeZone: zone });
    return true;
  } catch {
    return false;
  }
}

/**
 * The IANA zone a schedule's `spec` carries (`CRON_TZ=Zone 0 9 * * *` or
 * `0 9 * * * Zone`), or `undefined` for UTC-free specs (`every`, `at`, a bare
 * 5-field cron). Times of such a schedule are shown in THIS zone, not the browser's.
 */
export function specTimeZone(raw: unknown): string | undefined {
  const cron =
    raw !== null && typeof raw === "object" && "cron" in raw
      ? (raw as { cron: unknown }).cron
      : undefined;
  if (typeof cron !== "string") {
    return undefined;
  }
  const parts = cron.trim().split(/\s+/);
  const lead = parts[0]?.startsWith("CRON_TZ=") ? parts[0].slice(8) : undefined;
  const tail = parts.length > 5 ? parts[parts.length - 1] : undefined;
  const zone = lead ?? tail;
  return zone && validZone(zone) ? zone : undefined;
}

/** A run time in the schedule's own zone; the zone name is added when it is not the viewer's. */
export function formatInScheduleZone(
  ms: number,
  locale: string,
  zone: string | undefined,
  viewerZone: string
): string {
  const foreign = zone !== undefined && zone !== viewerZone;
  return new Date(ms).toLocaleString(locale, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    ...(zone ? { timeZone: zone } : {}),
    ...(foreign ? { timeZoneName: "short" } : {})
  });
}
