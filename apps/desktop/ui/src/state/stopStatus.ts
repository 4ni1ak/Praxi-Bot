import type { RunEntry, StopRecord } from "./runLedger.ts";

/**
 * What the user sees about their latest Stop (pure; `StopStatusNotice` renders it).
 *
 * Honest by construction: only runs POSITIVELY known to be terminal count as
 * stopped. `cancel_requested` is still "stopping"; `failed` stays visibly not
 * stopped; a Stop the daemon has not answered says so (and says what it will cancel
 * when it arrives); an expired Stop is shown as expired; a Stop whose target set the
 * daemon forgot before telling the app is "not confirmed", never "stopped".
 */

export type StopTone = "progress" | "success" | "warning" | "danger";

export type StopKind =
  /** Request sent, or the daemon answered and some runs are still winding down. */
  | "stopping"
  /** The daemon has not been reached: local input is halted, daemon runs are not confirmed. */
  | "unreachable"
  /** Some runs could not be cancelled (retrying; they never act meanwhile). */
  | "partial"
  /** The daemon forgot this Stop after telling the app its targets; only those still held are retried. */
  | "expired"
  /** The daemon forgot this Stop before telling the app what it targeted: runs that may be targets are paused. */
  | "unconfirmed"
  /** From before an app restart, never answered, nothing known to retry: press Stop again. */
  | "unconfirmable"
  /** Every targeted run is terminal. */
  | "stopped";

export interface StopStatus {
  readonly stopId: string;
  readonly kind: StopKind;
  readonly tone: StopTone;
  readonly total: number;
  readonly stopped: number;
  readonly stopping: number;
  readonly failed: number;
  /** Runs paused from acting because they may belong to an unconfirmed target set. */
  readonly quarantined: number;
  /**
   * The first delivery is still pending and will stop EVERY run open when it reaches
   * the daemon, including runs started meanwhile (the app had no run list when Stop
   * was pressed). `false`: it names only the runs the app knew.
   */
  readonly pendingSweep: boolean;
  /** Native input (bot overlay / takeover keys) was halted on this computer. */
  readonly localHalted: boolean;
  /** `false` when the daemon could not write the audit row for this Stop. */
  readonly audited: boolean;
  /** The user may dismiss it (nothing is in progress). */
  readonly dismissible: boolean;
  /** Unconfirmed: offers "Stop all now" / "Let them continue" (and a plain dismiss). */
  readonly recoverable?: boolean;
  readonly error: string | null;
}

/** How long a confirmed Stop stays visible. */
export const STOPPED_VISIBLE_MS = 6000;
/** How long an expired / unconfirmable notice stays without being dismissed. */
export const NOTICE_VISIBLE_MS = 20_000;

const OPEN: ReadonlySet<StopRecord["phase"]> = new Set(["queued", "in_flight", "retrying"]);

function chainHead(
  rec: Readonly<StopRecord>,
  byId: ReadonlyMap<string, Readonly<StopRecord>>
): { head: Readonly<StopRecord>; expired: boolean } {
  let head = rec;
  let expired = rec.phase === "expired";
  const seen = new Set<string>([rec.id]);
  while (head.successor) {
    const next = byId.get(head.successor);
    if (!next || seen.has(next.id)) {
      break;
    }
    seen.add(next.id);
    head = next;
    expired = expired || head.phase === "expired";
  }
  return { head, expired };
}

export function describeStop(input: {
  records: readonly Readonly<StopRecord>[];
  runs: readonly Readonly<RunEntry>[];
  /** Monotonic now (same clock as the ledger). */
  now: number;
  /** A daemon client is attached right now. */
  connected: boolean;
  dismissed?: ReadonlySet<string>;
}): StopStatus | null {
  const byId = new Map(input.records.map((r) => [r.id, r]));
  // The latest user decision (successors are not decisions of their own). An
  // unresolved unconfirmed Stop keeps its own status (and recovery) until it is
  // confirmed, released or dismissed: a later Stop never hides it.
  const roots = input.records.filter(
    (r) => r.kind === "all" && r.supersedes === null && !input.dismissed?.has(r.id)
  );
  const unresolved = roots.filter((r) => r.phase === "unconfirmed");
  const root = unresolved[unresolved.length - 1] ?? roots[roots.length - 1];
  if (!root) {
    return null;
  }
  const { head, expired } = chainHead(root, byId);
  const runs = new Map(input.runs.map((r) => [r.id, r]));
  const members = new Set<string>([...root.members, ...head.members]);
  let stopped = 0;
  let stopping = 0;
  let failed = 0;
  let error: string | null = null;
  for (const id of members) {
    const run = runs.get(id);
    if (!run || run.phase === "stopped") {
      stopped += 1;
    } else if (run.phase === "failed") {
      failed += 1;
      error = error ?? run.error;
    } else {
      stopping += 1;
    }
  }
  const quarantined = input.runs.filter((r) => r.quarantinedBy === root.id).length;
  const total = members.size;
  const audited = root.audited && head.audited;
  const base = {
    stopId: root.id,
    total,
    stopped,
    stopping,
    failed,
    quarantined,
    pendingSweep: !head.reached && head.scope === "all_existing" && OPEN.has(head.phase),
    localHalted: true,
    audited,
    error
  };
  const open = OPEN.has(head.phase);

  if (root.phase === "unconfirmed") {
    return { ...base, kind: "unconfirmed", tone: "warning", dismissible: false, recoverable: true };
  }
  if (head.phase === "unconfirmable") {
    return { ...base, kind: "unconfirmable", tone: "warning", dismissible: true };
  }
  if (root.phase === "released") {
    const age = input.now - (root.settledMono ?? input.now);
    return age > NOTICE_VISIBLE_MS
      ? null
      : { ...base, kind: "unconfirmed", tone: "warning", dismissible: true };
  }
  if (expired && (open || head.phase === "expired")) {
    // Expired with a successor still working, or expired with nothing left to retry.
    const age = input.now - (root.settledMono ?? input.now);
    if (!open && age > NOTICE_VISIBLE_MS) {
      return null;
    }
    return { ...base, kind: "expired", tone: "warning", dismissible: !open };
  }
  if (head.phase === "settled") {
    const age = input.now - (head.settledMono ?? Number.NEGATIVE_INFINITY);
    if (!(age <= STOPPED_VISIBLE_MS)) {
      return null;
    }
    return { ...base, kind: "stopped", tone: "success", dismissible: true };
  }
  if (!head.reached) {
    if (!input.connected || head.lastError !== null) {
      return { ...base, kind: "unreachable", tone: "warning", dismissible: false };
    }
    return { ...base, kind: "stopping", tone: "progress", dismissible: false };
  }
  if (failed > 0) {
    return { ...base, kind: "partial", tone: "danger", dismissible: false };
  }
  return { ...base, kind: "stopping", tone: "progress", dismissible: false };
}

/** i18n key of the title line for a status. */
export function stopTitleKey(status: StopStatus): string {
  if (status.kind === "unconfirmed" && status.dismissible) {
    return "stopStatus.title.released";
  }
  return `stopStatus.title.${status.kind}`;
}

/** i18n key + params of the detail line for a status. */
export function stopDetail(status: StopStatus): { key: string; params: Record<string, number> } {
  const params = {
    count: status.total,
    stopped: status.stopped,
    stopping: status.stopping,
    failed: status.failed,
    held: status.stopping + status.failed,
    quarantined: status.quarantined
  };
  switch (status.kind) {
    case "stopping":
      // Connected but the first delivery is still a sweep (the app had no fresh run
      // list): say what it will cancel before it arrives.
      if (status.pendingSweep) {
        return { key: "stopStatus.detail.stoppingSweep", params };
      }
      return status.total > 0
        ? { key: "stopStatus.detail.stoppingCount", params }
        : { key: "stopStatus.detail.stoppingNone", params };
    case "unreachable":
      // Tell the user exactly what the pending first delivery will cancel.
      return status.pendingSweep
        ? { key: "stopStatus.detail.unreachableSweep", params }
        : { key: "stopStatus.detail.unreachableListed", params };
    case "partial":
      return { key: "stopStatus.detail.partial", params };
    case "expired":
      return params.held > 0
        ? { key: "stopStatus.detail.expiredRetry", params }
        : { key: "stopStatus.detail.expiredNone", params };
    case "unconfirmed":
      return status.dismissible
        ? { key: "stopStatus.detail.released", params }
        : { key: "stopStatus.detail.unconfirmed", params };
    case "unconfirmable":
      return { key: "stopStatus.detail.unconfirmable", params };
    case "stopped":
      return status.total > 0
        ? { key: "stopStatus.detail.stoppedCount", params }
        : { key: "stopStatus.detail.stoppedNone", params };
  }
}

// ── widget (native only) ───────────────────────────────────────────────────────

/**
 * The widget has no daemon client: it can only tell whether native still has a
 * Stop nobody confirmed (`stopSeq > stopAckedSeq`). The main window acks a Stop
 * once every run it targeted is terminal. The cursors only move forward, so a stale
 * snapshot can never look like an ack, and a failed read is "could not confirm".
 */
export interface WidgetStopState {
  readonly kind: "none" | "pending" | "confirmed";
  /** Highest `stopSeq` / `stopAckedSeq` seen in a successful read. */
  readonly seq: number;
  readonly acked: number;
  /** The sequence a pending state waits for. */
  readonly pendingSeq: number;
  readonly since: number;
  /** The latest read failed while a Stop was pending. */
  readonly unknown: boolean;
}

export const WIDGET_STOP_INITIAL: WidgetStopState = {
  kind: "none",
  seq: 0,
  acked: 0,
  pendingSeq: 0,
  since: 0,
  unknown: false
};

export const WIDGET_CONFIRMED_MS = 4000;

export function nextWidgetStop(
  prev: WidgetStopState,
  snap: { stopSeq?: number; stopAckedSeq?: number } | null,
  now: number
): WidgetStopState {
  if (!snap || snap.stopSeq === undefined || snap.stopAckedSeq === undefined) {
    // A failed read proves nothing: keep the pending Stop, say it could not be confirmed.
    return prev.kind === "pending" ? { ...prev, unknown: true } : expire(prev, now);
  }
  const seq = Math.max(prev.seq, snap.stopSeq);
  const acked = Math.max(prev.acked, snap.stopAckedSeq);
  if (seq > acked) {
    return { kind: "pending", seq, acked, pendingSeq: seq, since: now, unknown: false };
  }
  if (prev.kind === "pending" && acked >= prev.pendingSeq) {
    return { kind: "confirmed", seq, acked, pendingSeq: prev.pendingSeq, since: now, unknown: false };
  }
  return expire({ ...prev, seq, acked }, now);
}

function expire(prev: WidgetStopState, now: number): WidgetStopState {
  if (prev.kind === "confirmed" && now - prev.since > WIDGET_CONFIRMED_MS) {
    return { ...prev, kind: "none" };
  }
  return prev;
}

/** The widget applies a snapshot's mode only when its revision is newer than the last applied. */
export function isNewerNativeSnapshot(
  lastRev: number,
  snap: { stateRev?: number; stopSeq?: number }
): boolean {
  return snap.stopSeq !== undefined && snap.stateRev !== undefined && snap.stateRev > lastRev;
}

/** What the widget shows: its Stop state plus the newest accepted native snapshot. */
export interface WidgetView<S> {
  readonly stop: WidgetStopState;
  readonly lastRev: number;
  readonly control: S;
}

/**
 * Every native read, event and command reply goes through here (null = failed read).
 * The mode is taken only from a newer revision; the Stop cursors only move forward.
 */
export function reduceWidget<S extends { stateRev?: number; stopSeq?: number; stopAckedSeq?: number }>(
  prev: WidgetView<S>,
  snap: S | null,
  now: number
): WidgetView<S> {
  const stop = nextWidgetStop(prev.stop, snap, now);
  if (snap && isNewerNativeSnapshot(prev.lastRev, snap)) {
    return { stop, lastRev: snap.stateRev as number, control: snap };
  }
  return stop === prev.stop ? prev : { ...prev, stop };
}
