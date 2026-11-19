import type { AdminCliLogin } from "../api/types.ts";

/** What the sign-in modal shows. `idle` = nothing started yet (or the engine cannot be driven). */
export type SignInPhase =
  | "idle"
  | "starting"
  | "needs_browser"
  | "needs_code"
  | "verifying"
  | "done"
  | "failed"
  | "timed_out"
  | "cancelled";

const PHASES: readonly SignInPhase[] = [
  "idle",
  "starting",
  "needs_browser",
  "needs_code",
  "verifying",
  "done",
  "failed",
  "timed_out",
  "cancelled"
];

export function signInPhase(job: Pick<AdminCliLogin, "state"> | null): SignInPhase {
  const s = job?.state as SignInPhase | undefined;
  return s && PHASES.includes(s) ? s : "idle";
}

/**
 * The model provider that a pasted API key signs in for, per engine: the one-click alternative to
 * the engine's own sign-in. Engines without a matching provider offer none.
 */
export function apiKeyProvider(engineId: string): string | null {
  switch (engineId) {
    case "claude":
      return "anthropic";
    case "codex":
      return "openai";
    default:
      return null;
  }
}
