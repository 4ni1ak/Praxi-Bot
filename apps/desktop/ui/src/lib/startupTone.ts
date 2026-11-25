import type { StartupStage } from "../api/connection.ts";

/**
 * Whether the start-up screen is describing something still in progress ("the service is starting",
 * "Windows is still launching it") or something that failed.  In progress is calm and neutral; only
 * a real failure uses the danger colour.
 */
export function startupTone(stage: StartupStage): "progress" | "problem" {
  return stage === "idle" || stage === "spawning" || stage === "waiting" || stage === "ready" ? "progress" : "problem";
}
