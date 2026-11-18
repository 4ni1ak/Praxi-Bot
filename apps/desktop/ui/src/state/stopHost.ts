import type { ControlModeName } from "../api/types.ts";
import type { NativeStopSnapshot, StopRecord, StopTransport } from "./runLedger.ts";

/** Small pure helpers shared by `StopRuntime` and the hosts. */

export interface NativeControlSnapshot extends NativeStopSnapshot {
  mode: ControlModeName;
  runGeneration?: number;
}

/** The transport a daemon client gives the ledger. */
export function stopTransport(client: {
  stopRuns: StopTransport["stopRuns"];
}): StopTransport {
  return { stopRuns: (body, signal) => client.stopRuns(body, signal) };
}

/**
 * Takeover override for a Stop: the viewer's Stop button always pauses the screen
 * takeover it shows; every other entry point only when a takeover is held (so a
 * plain Stop never latches the whole daemon paused).
 */
export function overrideOptionsFor(rec: Pick<StopRecord, "origin">): {
  onlyIfControlled: boolean;
} {
  return { onlyIfControlled: rec.origin !== "viewer" };
}
