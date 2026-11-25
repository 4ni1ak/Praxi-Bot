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
