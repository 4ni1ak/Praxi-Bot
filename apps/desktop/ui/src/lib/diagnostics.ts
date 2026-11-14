import type { DoctorCheck, DoctorStatus, RepairId } from "../api/types.ts";

/** How a check is shown: `neutral` is "not available here", never an error. */
export type CheckTone = "ok" | "warn" | "fail" | "neutral";

export function checkTone(status: DoctorStatus): CheckTone {
  switch (status) {
    case "ok":
      return "ok";
    case "warn":
      return "warn";
    case "fail":
      return "fail";
    default:
      return "neutral";
  }
}

const ORDER: Record<CheckTone, number> = { fail: 0, warn: 1, ok: 2, neutral: 3 };

/** Problems first, then the rest in the daemon's order (stable). */
export function sortChecks(checks: readonly DoctorCheck[]): DoctorCheck[] {
  return checks
    .map((c, i) => ({ c, i }))
    .sort((a, b) => ORDER[checkTone(a.c.status)] - ORDER[checkTone(b.c.status)] || a.i - b.i)
    .map(({ c }) => c);
}

export interface DiagnosticsSummary {
  fail: number;
  warn: number;
  ok: number;
  neutral: number;
  /** Worst tone present (`ok` when nothing is wrong). */
  overall: CheckTone;
}

export function summarize(checks: readonly DoctorCheck[]): DiagnosticsSummary {
  const s = { fail: 0, warn: 0, ok: 0, neutral: 0 };
  for (const c of checks) {
    s[checkTone(c.status)] += 1;
  }
  return { ...s, overall: s.fail > 0 ? "fail" : s.warn > 0 ? "warn" : "ok" };
}

export interface RepairOutcome {
  ok: boolean;
  text: string;
}

export interface DiagnosticsState {
  phase: "idle" | "loading" | "ready" | "error";
  checks: DoctorCheck[];
  error: string | null;
  /** The repair in flight (only one at a time). */
  running: RepairId | null;
  /** Last outcome per check id. */
  outcomes: Record<string, RepairOutcome>;
}

export const INITIAL_DIAGNOSTICS: DiagnosticsState = {
  phase: "idle",
  checks: [],
  error: null,
  running: null,
  outcomes: {}
};

export type DiagnosticsAction =
  | { type: "load_start" }
  | { type: "load_ok"; checks: DoctorCheck[] }
  | { type: "load_error"; error: string }
  | { type: "repair_start"; repair: RepairId }
  | { type: "repair_done"; checkId: string; outcome: RepairOutcome };

export function diagnosticsReducer(
  state: DiagnosticsState,
  action: DiagnosticsAction
): DiagnosticsState {
  switch (action.type) {
    case "load_start":
      return { ...state, phase: "loading", error: null };
    case "load_ok":
      return { ...state, phase: "ready", checks: action.checks, error: null };
    case "load_error":
      return { ...state, phase: "error", error: action.error };
    case "repair_start":
      // Ignore a second click while a repair runs.
      return state.running ? state : { ...state, running: action.repair };
    case "repair_done":
      return {
        ...state,
        running: null,
        outcomes: { ...state.outcomes, [action.checkId]: action.outcome }
      };
  }
}

export interface RepairButton {
  repair: RepairId;
  disabled: boolean;
  busy: boolean;
}

/**
 * The repair button of a check: only for a fixable warning / failure, and
 * disabled while any repair runs, the checks reload or the support report is
 * being made (`busy`: any daemon call of the card in flight).
 */
export function repairButton(
  check: DoctorCheck,
  state: Pick<DiagnosticsState, "phase" | "running">,
  busy: DiagnosticsTask | null = null
): RepairButton | null {
  const tone = checkTone(check.status);
  if (!check.repair || (tone !== "fail" && tone !== "warn")) {
    return null;
  }
  return {
    repair: check.repair,
    disabled: state.running !== null || state.phase === "loading" || busy !== null,
    busy: state.running === check.repair
  };
}

/** A daemon call of the card.  The daemon runs one diagnostics request at a
 * time (`GET /v1/doctor` and `GET /v1/doctor/report` share one slot and answer
 * 429 to an overlapping call), and a repair reloads the checks afterwards, so
 * the card runs at most one of these at once. */
export type DiagnosticsTask = "load" | "copy" | "repair";

/**
 * Serializes the card's daemon calls: a task that starts while another runs
 * is refused (not queued) and never reaches the daemon.  `onChange` reports
 * the task in flight (`null` when idle) so the buttons can be disabled.
 */
export class DiagnosticsQueue {
  private busy: DiagnosticsTask | null = null;
  private readonly onChange: (busy: DiagnosticsTask | null) => void;

  constructor(onChange: (busy: DiagnosticsTask | null) => void = () => {}) {
    this.onChange = onChange;
  }

  /** The task in flight, if any. */
  get current(): DiagnosticsTask | null {
    return this.busy;
  }

  /** Run `fn` as `task` unless another task runs: `{ started: false }` then. */
  async run<T>(
    task: DiagnosticsTask,
    fn: () => Promise<T>
  ): Promise<{ started: false } | { started: true; value: T }> {
    if (this.busy !== null) {
      return { started: false };
    }
    this.busy = task;
    this.onChange(task);
    try {
      return { started: true, value: await fn() };
    } finally {
      this.busy = null;
      this.onChange(null);
    }
  }
}

export interface DiagnosticsControls {
  rerunDisabled: boolean;
  copyDisabled: boolean;
  /** Spinner on Copy support report. */
  copying: boolean;
  /** Spinner on Run again. */
  loading: boolean;
}

/** Which header actions are usable while `busy` runs (all of them compete
 * for the daemon's single diagnostics slot). */
export function controls(connected: boolean, busy: DiagnosticsTask | null): DiagnosticsControls {
  return {
    rerunDisabled: !connected || busy !== null,
    copyDisabled: !connected || busy !== null,
    copying: busy === "copy",
    loading: busy === "load"
  };
}

/** Which heading the card shows. */
export type DiagnosticsHeading = "title" | "running" | "error" | "empty" | "allGood" | "problems";

export function heading(
  connected: boolean,
  state: Pick<DiagnosticsState, "phase" | "checks">,
  problems: number
): DiagnosticsHeading {
  if (!connected) {
    return "title";
  }
  if (state.checks.length === 0) {
    switch (state.phase) {
      case "error":
        return "error";
      case "ready":
        return "empty";
      default:
        return "running";
    }
  }
  return problems === 0 ? "allGood" : "problems";
}
