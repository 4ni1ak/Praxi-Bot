import type { RunEntry, StopRecord } from "./runLedger.ts";

/**
 * What the user sees about their latest Stop (pure; `StopStatusNotice` renders it).
 *
 * Honest by construction: only terminal runs count as stopped. `cancel_requested`
 * is still "stopping"; `failed` stays visibly not stopped; a Stop the daemon has
 * not answered says so; an expired Stop is shown as expired.
 */

export type StopTone = "progress" | "success" | "warning" | "danger";

export type StopKind =
  /** Request sent, or the daemon answered and some runs are still winding down. */
  | "stopping"
  /** The daemon has not been reached: local input is halted, daemon runs are not confirmed. */
  | "unreachable"
  /** Some runs could not be cancelled (retrying; they never act meanwhile). */
  | "partial"
  /** The daemon forgot this Stop; only the runs still held are retried under a new id. */
  | "expired"
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
  /** Native input (bot overlay / takeover keys) was halted on this computer. */
  readonly localHalted: boolean;
  /** `false` when the daemon could not write the audit row for this Stop. */
  readonly audited: boolean;
  /** The user may dismiss it (nothing is in progress). */
  readonly dismissible: boolean;
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
  // The latest user decision (successors are not decisions of their own).
  const roots = input.records.filter((r) => r.kind === "all" && r.supersedes === null);
  const root = roots[roots.length - 1];
  if (!root || input.dismissed?.has(root.id)) {
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
  const total = members.size;
  const audited = root.audited && head.audited;
  const base = {
    stopId: root.id,
    total,
    stopped,
    stopping,
    failed,
    localHalted: true,
    audited,
    error
  };
  const open = OPEN.has(head.phase);

  if (head.phase === "unconfirmable") {
    return { ...base, kind: "unconfirmable", tone: "warning", dismissible: true };
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
  return `stopStatus.title.${status.kind}`;
}

/** i18n key + params of the detail line for a status. */
export function stopDetail(status: StopStatus): { key: string; params: Record<string, number> } {
  const params = {
    count: status.total,
    stopped: status.stopped,
    stopping: status.stopping,
    failed: status.failed,
    held: status.stopping + status.failed
  };
  switch (status.kind) {
    case "stopping":
      return status.total > 0
        ? { key: "stopStatus.detail.stoppingCount", params }
        : { key: "stopStatus.detail.stoppingNone", params };
    case "unreachable":
      return { key: "stopStatus.detail.unreachable", params };
    case "partial":
      return { key: "stopStatus.detail.partial", params };
    case "expired":
      return params.held > 0
        ? { key: "stopStatus.detail.expiredRetry", params }
        : { key: "stopStatus.detail.expiredNone", params };
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
 * once every run it targeted is terminal.
 */
export type WidgetStopState =
  | { kind: "none" }
  | { kind: "pending"; seq: number }
  | { kind: "confirmed"; seq: number; since: number };

export const WIDGET_CONFIRMED_MS = 4000;

export function nextWidgetStop(
  prev: WidgetStopState,
  snap: { stopSeq?: number; stopAckedSeq?: number },
  now: number
): WidgetStopState {
  const seq = snap.stopSeq ?? 0;
  const acked = snap.stopAckedSeq ?? seq;
  if (seq > acked) {
    return prev.kind === "pending" && prev.seq === seq ? prev : { kind: "pending", seq };
  }
  if (prev.kind === "pending") {
    return { kind: "confirmed", seq: prev.seq, since: now };
  }
  if (prev.kind === "confirmed" && now - prev.since <= WIDGET_CONFIRMED_MS) {
    return prev;
  }
  return { kind: "none" };
}
