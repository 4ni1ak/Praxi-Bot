import type {
  StopRunResult,
  StopRunsInput,
  StopRunsResponse
} from "../api/types.ts";
import {
  type LedgerStorage,
  type NativeStopSnapshot,
  RunLedger,
  type StopTransport
} from "./runLedger.ts";

/**
 * Test doubles for the Stop ledger (used by *.test.ts only): a manual clock, a
 * native control state machine with `stopSeq` / tokens / acks like `control.rs`,
 * and a daemon that implements `POST /v1/runs/stop` the way PR A does (target set
 * fixed at first receipt, repeats idempotent, `cancel_requested` for live runs,
 * `expired` for a forgotten id). Requests are captured and answered when the test
 * says so, so every interleaving is explicit.
 */

export class FakeClock {
  private t = 0;
  private seq = 0;
  private readonly timers = new Map<number, { at: number; fn: () => void }>();

  now = (): number => this.t;

  set = (fn: () => void, ms: number): number => {
    this.seq += 1;
    this.timers.set(this.seq, { at: this.t + ms, fn });
    return this.seq;
  };

  clear = (handle: unknown): void => {
    this.timers.delete(handle as number);
  };

  /** Advance time, firing due timers in order. */
  advance(ms: number): void {
    const end = this.t + ms;
    for (;;) {
      let next: [number, { at: number; fn: () => void }] | null = null;
      for (const entry of this.timers) {
        if (entry[1].at <= end && (!next || entry[1].at < next[1].at)) {
          next = entry;
        }
      }
      if (!next) {
        break;
      }
      this.timers.delete(next[0]);
      this.t = next[1].at;
      next[1].fn();
    }
    this.t = end;
  }

  pending(): number {
    return this.timers.size;
  }
}

/** Native control machine: user stops raise `stopSeq`, finishes do not. */
export class FakeNative {
  stopSeq = 0;
  stopAckedSeq = 0;
  stateRev = 0;
  runGeneration = 0;
  mode: "idle" | "bot_driving" | "human_driving" | "paused" = "idle";
  lastStopToken: string | null = null;
  readonly tokens: (string | null)[] = [];
  readonly acks: number[] = [];
  /** Snapshots emitted as events (the host routes them to the ledger). */
  readonly events: NativeStopSnapshot[] = [];
  /** Hold `stop_bot_run` replies until released (events are still emitted). */
  holdReplies = false;
  /**
   * Queue `stop_bot_run` invocations instead of executing them at once (the command
   * runs on the native side later; `runCommands()` executes them in order).
   */
  deferCommands = false;
  private readonly held: (() => void)[] = [];
  private readonly commands: (() => void)[] = [];

  snapshot(): NativeStopSnapshot & { mode: string; runGeneration: number } {
    return {
      mode: this.mode,
      runGeneration: this.runGeneration,
      stateRev: this.stateRev,
      stopSeq: this.stopSeq,
      lastStopToken: this.lastStopToken,
      stopAckedSeq: this.stopAckedSeq
    };
  }

  /** `stop_all(User{token})`: hotkey listener, tray, widget (no token) or the ledger (token). */
  userStop(token: string | null): NativeStopSnapshot {
    this.mode = "idle";
    this.runGeneration += 1;
    this.stopSeq += 1;
    this.stateRev += 1;
    this.lastStopToken = token;
    this.tokens.push(token);
    const snap = this.snapshot();
    this.events.push(snap);
    return snap;
  }

  /** `finish_run(expected)`: Idle without raising `stopSeq`. */
  finish(expected: number): NativeStopSnapshot {
    if (expected === this.runGeneration) {
      this.mode = "idle";
      this.runGeneration += 1;
      this.stateRev += 1;
      this.events.push(this.snapshot());
    }
    return this.snapshot();
  }

  /** `start_bot_run`: native drives a new run (new generation). */
  start(): NativeStopSnapshot & { mode: string; runGeneration: number } {
    this.mode = "bot_driving";
    this.runGeneration += 1;
    this.stateRev += 1;
    const snap = this.snapshot();
    this.events.push(snap);
    return snap;
  }

  /** `take_the_wheel` (human input). */
  takeWheel(): NativeStopSnapshot {
    this.mode = "human_driving";
    this.stateRev += 1;
    const snap = this.snapshot();
    this.events.push(snap);
    return snap;
  }

  /** Any other emitting change (step text, hotkey apply, refused action). */
  touch(): NativeStopSnapshot {
    this.stateRev += 1;
    const snap = this.snapshot();
    this.events.push(snap);
    return snap;
  }

  port() {
    return {
      stopRun: (token: string): Promise<NativeStopSnapshot> =>
        new Promise((resolve) => {
          const execute = () => {
            const snap = this.userStop(token);
            if (this.holdReplies) {
              this.held.push(() => resolve(snap));
            } else {
              resolve(snap);
            }
          };
          if (this.deferCommands) {
            this.commands.push(execute);
          } else {
            execute();
          }
        }),
      ackStop: async (seq: number) => {
        this.acks.push(seq);
        this.stopAckedSeq = Math.max(this.stopAckedSeq, Math.min(seq, this.stopSeq));
        return this.snapshot();
      }
    };
  }

  /** Execute queued `stop_bot_run` commands natively (events emitted, replies per `holdReplies`). */
  runCommands(): void {
    for (const c of this.commands.splice(0)) {
      c();
    }
  }

  releaseReplies(): void {
    for (const r of this.held.splice(0)) {
      r();
    }
  }
}

type DaemonRun = "queued" | "running" | "waiting_approval" | "paused" | "cancelled" | "succeeded";

interface Pending {
  captured?: StopRunsResponse;
  body: StopRunsInput;
  signal: AbortSignal;
  resolve: (r: StopRunsResponse) => void;
  reject: (e: unknown) => void;
  settled: boolean;
}

const TERMINAL: ReadonlySet<DaemonRun> = new Set(["cancelled", "succeeded"]);

/** The daemon side of `POST /v1/runs/stop`, plus `GET /v1/runs`. */
export class FakeDaemon {
  readonly runs = new Map<string, DaemonRun>();
  /** Live runs whose cancel token fired but which have not ended yet. */
  readonly tokenFired = new Set<string>();
  /** Runs whose cancel fails (until removed). */
  readonly failCancel = new Set<string>();
  private readonly book = new Map<string, Set<string>>();
  private readonly forgotten = new Set<string>();
  readonly requests: Pending[] = [];
  /** Order in which the daemon FIRST processed each stop id (the oracle for "existed at Stop"). */
  readonly firstReceipt: string[] = [];
  audited = true;

  start(id: string, state: DaemonRun = "running"): void {
    this.runs.set(id, state);
  }

  /** The run's task observed its cancel token and ended. */
  end(id: string, state: DaemonRun = "cancelled"): void {
    this.runs.set(id, state);
    this.tokenFired.delete(id);
  }

  /**
   * A genuine daemon restart: a fresh process, nothing about earlier Stops is known
   * (neither target sets nor seen ids), so a repeat is a first receipt again.
   */
  restart(): void {
    this.book.clear();
    this.forgotten.clear();
    this.tokenFired.clear();
  }

  /** TTL / eviction within ONE daemon lifetime: target sets gone, ids remembered (`expired`). */
  forgetBook(): void {
    for (const id of this.book.keys()) {
      this.forgotten.add(id);
    }
    this.book.clear();
  }

  transport(): StopTransport {
    return {
      stopRuns: (body, signal) =>
        new Promise<StopRunsResponse>((resolve, reject) => {
          const p: Pending = { body, signal, resolve, reject, settled: false };
          this.requests.push(p);
          signal.addEventListener("abort", () => {
            if (!p.settled) {
              p.settled = true;
              reject(new Error("aborted"));
            }
          });
        })
    };
  }

  /** Requests not answered yet. */
  open(): Pending[] {
    return this.requests.filter((r) => !r.settled);
  }

  /** Process request `i` NOW (fixes T if first receipt) and deliver the reply. */
  deliver(i = this.requests.length - 1): StopRunsResponse {
    const p = this.requests[i];
    if (!p || p.settled) {
      throw new Error(`request ${i} is not open`);
    }
    const resp = this.process(p.body);
    p.settled = true;
    p.resolve(resp);
    return resp;
  }

  /**
   * Process request `i` NOW (fixing T if first receipt) but hold the reply; deliver it
   * later with `release(i)`. Processing and reply delivery are separate steps.
   */
  processOnly(i = this.requests.length - 1): void {
    const p = this.requests[i];
    if (!p || p.settled || p.captured) {
      throw new Error(`request ${i} is not open`);
    }
    p.captured = this.process(p.body);
  }

  release(i: number): void {
    const p = this.requests[i];
    if (!p || p.settled || !p.captured) {
      throw new Error(`request ${i} has no captured reply`);
    }
    p.settled = true;
    p.resolve(p.captured);
  }

  /** Process request `i` NOW but lose the reply (the client sees a transport error). */
  processAndLose(i = this.requests.length - 1): void {
    const p = this.requests[i];
    if (!p || p.settled) {
      throw new Error(`request ${i} is not open`);
    }
    this.process(p.body);
    p.settled = true;
    p.reject(new Error("connection reset"));
  }

  fail(i = this.requests.length - 1): void {
    const p = this.requests[i];
    if (!p || p.settled) {
      throw new Error(`request ${i} is not open`);
    }
    p.settled = true;
    p.reject(new Error("network down"));
  }

  list(): { id: string; state: string; bot_id: string }[] {
    return [...this.runs].map(([id, state]) => ({ id, state, bot_id: "b1" }));
  }

  private process(body: StopRunsInput): StopRunsResponse {
    let targets = this.book.get(body.stop_id);
    let firstSeen = false;
    if (!targets) {
      if (this.forgotten.has(body.stop_id)) {
        return { stop_id: body.stop_id, first_seen: false, expired: true, results: [], audited: this.audited };
      }
      firstSeen = true;
      targets = new Set(body.run_ids ?? []);
      if (body.scope === "all_existing") {
        for (const [id, state] of this.runs) {
          if (!TERMINAL.has(state)) {
            targets.add(id);
          }
        }
      }
      this.book.set(body.stop_id, targets);
      this.firstReceipt.push(body.stop_id);
    }
    const results: StopRunResult[] = [];
    for (const id of targets) {
      const state = this.runs.get(id);
      if (!state) {
        results.push({ run_id: id, outcome: "not_found", state: null });
        continue;
      }
      if (TERMINAL.has(state)) {
        results.push({
          run_id: id,
          outcome: state === "cancelled" && !firstSeen ? "cancelled" : "already_terminal",
          state
        });
        continue;
      }
      if (this.failCancel.has(id)) {
        results.push({ run_id: id, outcome: "failed", state, error: "cancel refused" });
        continue;
      }
      if (state === "running") {
        this.tokenFired.add(id);
        results.push({ run_id: id, outcome: "cancel_requested", state });
        continue;
      }
      this.runs.set(id, "cancelled");
      results.push({ run_id: id, outcome: "cancelled", state: "cancelled" });
    }
    return { stop_id: body.stop_id, first_seen: firstSeen, audited: this.audited, results };
  }
}

export class MemoryStorage implements LedgerStorage {
  value: string | null = null;
  load = (): string | null => this.value;
  save = (v: string): void => {
    this.value = v;
  };
}

/** Let promise callbacks run. */
export async function flush(): Promise<void> {
  for (let i = 0; i < 5; i += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

let idCounter = 0;

export function makeLedger(opts: {
  clock?: FakeClock;
  native?: FakeNative;
  session?: string;
  storage?: LedgerStorage;
} = {}) {
  const clock = opts.clock ?? new FakeClock();
  const native = opts.native ?? new FakeNative();
  const ledger = new RunLedger({
    newId: () => {
      idCounter += 1;
      return `00000000-0000-4000-8000-${String(idCounter).padStart(12, "0")}`;
    },
    clock,
    native: native.port(),
    session: opts.session ?? "session-1",
    storage: opts.storage
  });
  return { ledger, clock, native };
}
