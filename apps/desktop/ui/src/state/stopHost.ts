import type { ControlModeName } from "../api/types.ts";
import type {
  ClientHandle,
  ListedRun,
  NativeStopSnapshot,
  RunLedger,
  StopRecord,
  StopTransport
} from "./runLedger.ts";

/**
 * Glue between the app (React hosts) and the Stop ledger. Pure functions over
 * injected parts, so the wiring the hosts rely on is unit tested
 * (`stopHost.test.ts`); the hosts only call these.
 */

export interface NativeModeSink {
  onNativeState(mode: ControlModeName, generation?: number): Promise<void>;
}

export interface NativeControlSnapshot extends NativeStopSnapshot {
  mode: ControlModeName;
  runGeneration?: number;
}

/**
 * Every native snapshot (event, command reply, initial read) goes to the ledger
 * first (Stops, acks); the bridge only sees modes newer than the last applied.
 */
export function routeNativeSnapshot(
  ledger: RunLedger,
  bridge: NativeModeSink | null,
  snap: NativeControlSnapshot
): void {
  const fresh = ledger.onNativeSnapshot(snap);
  if (fresh && bridge) {
    void bridge.onNativeState(snap.mode, snap.runGeneration);
  }
}

export interface RestoreSink {
  restoreDriving(): Promise<void>;
}

/**
 * `GET /v1/runs` on this client: which runs are live (and confirm stop-held ones
 * that ended). Pending Stops were already flushed by `attachClient`, so a run seen
 * here while a sweeping Stop is unanswered stays quarantined (cannot act).
 */
export async function restoreRuns(
  ledger: RunLedger,
  handle: ClientHandle,
  client: { listRuns(): Promise<readonly ListedRun[]> },
  bridge: RestoreSink | null
): Promise<void> {
  ledger.beginList(handle);
  let list: readonly ListedRun[];
  try {
    list = await client.listRuns();
  } catch {
    return; // the next heartbeat retries while the client is not synced
  }
  ledger.applyList(handle, list);
  if (bridge && handle.live && ledger.client() === handle) {
    await bridge.restoreDriving();
  }
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
