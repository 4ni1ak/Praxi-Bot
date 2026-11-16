import type { ControlStateDto, ReleaseRequest } from "../api/types.ts";
import {
  type ForeignPaused,
  releaseConfirmedBy,
  sameForeignPaused,
  snapshotPausedTakeoverId
} from "./liveViewState.ts";
import { humanLine } from "./humanError.ts";

export interface ReleaseDeps {
  /** `POST /v1/control/release` (through the control bridge when one exists). */
  release(req: ReleaseRequest): Promise<ControlStateDto | null>;
  getControlState(): Promise<ControlStateDto>;
}

export type ReleaseOutcome =
  | { kind: "confirmed"; dto: ControlStateDto }
  | {
      kind: "failed";
      code: string;
      message: string;
      /** Fresh daemon state when it could be read (takeover still reported). */
      dto: ControlStateDto | null;
    };

function describeError(err: unknown, fallbackCode: string) {
  const code =
    err !== null &&
    typeof err === "object" &&
    typeof (err as { code?: unknown }).code === "string"
      ? (err as { code: string }).code
      : fallbackCode;
  const message = humanLine(err);
  return { code, message };
}

/**
 * Hand a takeover back. The release counts as confirmed only when authoritative
 * daemon state no longer shows that takeover: a rejected POST is verified with a
 * fresh `GET /v1/control/state` (the response may have been lost after the daemon
 * applied it), and anything else is a failure the user can retry with the same
 * request id.
 */
export async function runRelease(
  deps: ReleaseDeps,
  payload: ReleaseRequest
): Promise<ReleaseOutcome> {
  let response: ControlStateDto | null = null;
  let postError: unknown = null;
  try {
    response = await deps.release(payload);
  } catch (err) {
    postError = err;
  }
  if (response && releaseConfirmedBy(response, payload.takeover_id)) {
    return { kind: "confirmed", dto: response };
  }
  let fresh: ControlStateDto | null = null;
  try {
    fresh = await deps.getControlState();
  } catch {
    fresh = null;
  }
  if (fresh && releaseConfirmedBy(fresh, payload.takeover_id)) {
    return { kind: "confirmed", dto: fresh };
  }
  if (postError !== null) {
    return {
      kind: "failed",
      ...describeError(postError, "release_failed"),
      dto: fresh ?? response
    };
  }
  return {
    kind: "failed",
    code: "release_unconfirmed",
    message: "release not confirmed by daemon state",
    dto: fresh ?? response
  };
}

export type ClearPauseOutcome =
  | { kind: "cleared"; dto: ControlStateDto }
  | { kind: "aborted"; fresh: ControlStateDto | null }
  | { kind: "failed"; code: string; message: string };

/** What the user is clearing; every kind is verified against fresh state first. */
export type ClearTarget =
  | { kind: "own"; takeoverId: string }
  | { kind: "foreign"; expected: ForeignPaused }
  | { kind: "latch" };

/**
 * Clear a pause only if the state the user saw is still the daemon's state:
 * - `own`: the same takeover id is still paused and ours -> `POST /control/recover`;
 * - `foreign`: still paused with the same owner, target and reason (the id is
 *   redacted) -> `POST /control/recover` (local user only; the daemon's recover
 *   does not check the caller against the owner);
 * - `latch`: no takeover at all, a plain user pause -> the ordinary
 *   `POST /control/release` (ADR 0029); `recover` only as the fallback for the
 *   unreadable-latch case the daemon requires it for.
 * Anything that changed in between aborts without posting; unreadable state
 * fails closed.
 */
export async function runClearPause(
  deps: {
    getControlState(): Promise<ControlStateDto>;
    recover(): Promise<ControlStateDto>;
    releaseLatch(): Promise<ControlStateDto>;
  },
  target: ClearTarget | null
): Promise<ClearPauseOutcome> {
  if (target === null) {
    return { kind: "aborted", fresh: null };
  }
  let fresh: ControlStateDto;
  try {
    fresh = await deps.getControlState();
  } catch (err) {
    return { kind: "failed", ...describeError(err, "recover_failed") };
  }
  const still =
    target.kind === "own"
      ? snapshotPausedTakeoverId(fresh) === target.takeoverId
      : target.kind === "foreign"
        ? sameForeignPaused(fresh, target.expected)
        : (fresh.takeover ?? null) === null;
  if (!still) {
    return { kind: "aborted", fresh };
  }
  try {
    if (target.kind === "latch") {
      try {
        return { kind: "cleared", dto: await deps.releaseLatch() };
      } catch (releaseErr) {
        // A refusal can be the race signal (a peer just took control): re-read
        // and fall back to `recover` (which interrupts whoever holds control)
        // only if there is still no takeover and the latch is still paused.
        let again: ControlStateDto;
        try {
          again = await deps.getControlState();
        } catch {
          return { kind: "failed", ...describeError(releaseErr, "recover_failed") };
        }
        if ((again.takeover ?? null) !== null || again.latched !== "paused") {
          return { kind: "aborted", fresh: again };
        }
        try {
          return { kind: "cleared", dto: await deps.recover() };
        } catch {
          return { kind: "failed", ...describeError(releaseErr, "recover_failed") };
        }
      }
    }
    return { kind: "cleared", dto: await deps.recover() };
  } catch (err) {
    return { kind: "failed", ...describeError(err, "recover_failed") };
  }
}
