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

/** Reason codes the daemon serves for a missing screen (a fixed set, no free text). */
export const NO_SCREEN_CODES = [
  "desktop_off",
  "unsupported_platform",
  "wayland",
  "no_display",
  "screen_unavailable",
  "start_failed",
  "starting",
  "timed_out"
] as const;

/** i18n key of the localized sentence for a no-screen code (unknown codes stay generic). */
export function noScreenDetailKey(code: string): string {
  return (NO_SCREEN_CODES as readonly string[]).includes(code)
    ? `liveView.noScreen.${code}`
    : "liveView.noScreen.unknown";
}

/** The code of the desktop's "why there is no screen" note, when the host recorded one. */
export function noScreenCode(
  availability: ReadonlyArray<{ kind: string; code: string }>
): string | null {
  const note = availability.find((n) => n.kind === "desktop") ?? availability[0];
  return note?.code ? note.code : null;
}

export interface ComputerPanelInput {
  hasTarget: boolean;
  /** The selected target can be viewed right now. */
  available: boolean;
  /** Why the selected target cannot be viewed (`liveView.reasons.<reason>`). */
  reason: string | null;
  viewState: string;
  hasFrame: boolean;
  targetsStatus: TargetsStatus;
  /** Code of the host's "no screen" note, if any. */
  noScreen: string | null;
}

/** One localizable line: an i18n key plus the key of its `{{detail}}`, if any. */
export interface PanelText {
  key: string;
  detailKey?: string;
}

/**
 * What the right panel's computer block says, from the query status plus the
 * known-target and session state, so the header pill and the preview can never
 * disagree (a known available screen with no first frame is "loading", never
 * "no screens").
 */
export function computerPanel(input: ComputerPanelInput): {
  pill: PanelText;
  preview: PanelText;
} {
  const none = (): { pill: PanelText; preview: PanelText } => {
    if (input.targetsStatus === "failed") {
      return {
        pill: { key: "liveView.states.queryFailed" },
        preview: { key: "liveView.queryFailed" }
      };
    }
    if (input.targetsStatus === "unknown") {
      return {
        pill: { key: "liveView.states.checking" },
        preview: { key: "liveView.checkingTargets" }
      };
    }
    if (input.noScreen) {
      const text = {
        key: "liveView.panelSummary.noScreenReason",
        detailKey: noScreenDetailKey(input.noScreen)
      };
      return { pill: text, preview: text };
    }
    return {
      pill: { key: "liveView.panelSummary.empty" },
      preview: { key: "liveView.noTargets" }
    };
  };
  if (!input.hasTarget) {
    return none();
  }
  const ready = isReadyIdle(input.viewState, input.hasFrame, input.available);
  const pill: PanelText = {
    key: ready
      ? "liveView.panelSummary.ready"
      : input.viewState === "active"
        ? "liveView.panelSummary.active"
        : input.viewState === "acquiring"
          ? "liveView.panelSummary.acquiring"
          : input.viewState === "paused"
            ? "liveView.panelSummary.paused"
            : !input.available
              ? "liveView.panelSummary.unavailable"
              : "liveView.panelSummary.watch"
  };
  const preview: PanelText = input.reason
    ? { key: `liveView.reasons.${input.reason}` }
    : { key: "liveView.loadingPreview" };
  return { pill, preview };
}

/**
 * An available screen nobody is watching is "ready", not "watching live":
 * capture only starts when a viewer subscribes.
 */
export function isReadyIdle(
  viewState: string,
  hasFrame: boolean,
  available: boolean
): boolean {
  return available && viewState === "watch" && !hasFrame;
}
