/**
 * An approval the owner skipped by writing a new message instead of answering it. The daemon
 * records it as a denial with this reason (`REDIRECTED_REASON` in crates/agent-core/src/approval.rs:
 * keep both texts equal), and the chat words it as "Skipped", not "Blocked by you".
 */
import type { TimelineItem } from "../state/timeline.ts";

export const REDIRECT_DENY_REASON = "denied: the owner wrote a new message instead";

export function isRedirectDenial(reason: string | null | undefined): boolean {
  return reason === REDIRECT_DENY_REASON;
}

/** True when a card of this run was skipped by a new message (the run was stopped for that, not by Stop). */
export function runWasRedirected(items: readonly TimelineItem[]): boolean {
  return items.some((i) => i.kind === "tool" && isRedirectDenial(i.denyReason));
}
