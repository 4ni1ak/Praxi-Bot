export type SiteRuleExpiry = "never" | "day" | "week" | "month";

/** Hours until a rule expires, or undefined for a rule that never does. */
export function siteRuleExpiryHours(e: SiteRuleExpiry): number | undefined {
  switch (e) {
    case "day":
      return 24;
    case "week":
      return 24 * 7;
    case "month":
      return 24 * 30;
    default:
      return undefined;
  }
}

/**
 * What a rule or suggestion names. A site is its host. A program is shown by file name in
 * lists, but a confirmation must show the FULL path: two programs can share a file name.
 */
export function siteRuleTarget(kind: string, key: string, full = false): string {
  if (kind !== "app" || full) return key;
  return key.split(/[\\/]/).pop() || key;
}
