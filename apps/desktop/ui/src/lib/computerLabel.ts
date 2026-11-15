// What the right panel's computer block says about the screens it knows.
// "No screen yet" is only true after a successful, empty answer; before that
// answer (or when it failed) the panel must not claim there is nothing.

export type TargetsStatus = "unknown" | "ok" | "failed";

export type ComputerFooter =
  | "interactive"
  | "viewOnly"
  | "unavailable"
  | "noScreen"
  | "checking"
  | "queryFailed";

export function computerFooter(
  summary: { available: boolean; canInput: boolean } | null,
  status: TargetsStatus
): ComputerFooter {
  if (summary) {
    if (summary.available) {
      return summary.canInput ? "interactive" : "viewOnly";
    }
    return "unavailable";
  }
  if (status === "failed") {
    return "queryFailed";
  }
  return status === "ok" ? "noScreen" : "checking";
}

/** i18n key of each footer variant. */
export const COMPUTER_FOOTER_KEY: Record<ComputerFooter, string> = {
  interactive: "liveView.interactive",
  viewOnly: "liveView.viewOnly",
  unavailable: "liveView.states.unavailable",
  noScreen: "liveView.states.noScreen",
  checking: "liveView.states.checking",
  queryFailed: "liveView.states.queryFailed"
};

/** Placeholder text key when no target (or no reason) is shown. */
export function computerPlaceholderKey(status: TargetsStatus): string {
  return status === "failed"
    ? "liveView.queryFailed"
    : status === "ok"
      ? "liveView.noTargets"
      : "liveView.checkingTargets";
}
