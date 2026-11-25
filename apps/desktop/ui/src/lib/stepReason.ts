import type { ToolItem } from "../state/timeline.ts";
import { isRedirectDenial } from "./approvalSkip.ts";

/**
 * Why a step did not finish, in one short phrase the row can show ("Timed out after 30 s",
 * "Blocked by you"). Pure: the component turns the kind into words. `other` means there is no
 * short honest reason, so the row falls back to the plain error title.
 */
export type StepReason =
  | { kind: "timeout"; seconds?: number }
  | { kind: "blocked" }
  | { kind: "skipped" }
  | { kind: "noAnswer" }
  | { kind: "stopped" }
  | { kind: "other" };

const TIMEOUT = /timed? ?out|etimedout|deadline/i;

export function stepReason(
  tool: Pick<ToolItem, "status" | "error" | "durationMs"> & Partial<Pick<ToolItem, "denyReason">>,
  orphan: boolean
): StepReason | null {
  if (orphan) return { kind: "stopped" };
  // The owner wrote a new message instead of answering the approval: skipped, not refused.
  if (tool.status === "denied" && isRedirectDenial(tool.denyReason)) return { kind: "skipped" };
  if (tool.status === "denied") return { kind: "blocked" };
  if (tool.status === "expired") return { kind: "noAnswer" };
  if (tool.status !== "error") return null;
  if (tool.error && TIMEOUT.test(tool.error)) {
    const seconds = tool.durationMs !== undefined ? Math.round(tool.durationMs / 1000) : undefined;
    return seconds && seconds > 0 ? { kind: "timeout", seconds } : { kind: "timeout" };
  }
  return { kind: "other" };
}
