import type {
  StopOutcome,
  StopRunsInput,
  StopRunsResponse,
  StopScope
} from "../api/types.ts";

/**
 * The app's single owner of run lifecycle and Stop decisions (card 237, PR C).
 *
 * Pure TypeScript: no React, no globals. Time, ids, timers, native commands, the
 * daemon transport and storage are injected, so every rule below is unit tested.
 *
 * - A Stop is ONE user decision with ONE `stop_id`. The same id is the daemon
 *   `stop_id` and the native `stop_bot_run` token, and every retry reuses it.
 * - The daemon decides which runs a Stop targets (`POST /v1/runs/stop` fixes the
 *   target set at first receipt). No clock is ever compared with the daemon's.
 * - A run a Stop holds never acts again (`mayAct` is false) until it is terminal.
 *   A failed cancel is retried, never re-tracked.
 * - Native tells the ledger a Stop happened (`stopSeq`); a normal finish never
 *   raises it. `ack_stop` is sent only once every run a Stop targeted is terminal.
 * - Daemon clients come and go (`attachClient` / `detachClient`); Stop records
 *   outlive them and are sent again to the next client.
 */

/** A first attempt that has not reached the daemon by then is narrowed to the runs the app knew. */
export const STOP_SWEEP_WINDOW_MS = 2000;
/** A UI Stop and a native Stop this close together are the same key press. */
export const STOP_COALESCE_MS = 250;
/** One attempt of `POST /v1/runs/stop` is abandoned (and retried) after this. */
export const STOP_ATTEMPT_TIMEOUT_MS = 3000;
/** Retry delays; the last one repeats for as long as a targeted run is not terminal. */
export const STOP_RETRY_MS: readonly number[] = [500, 1000, 2000, 5000];
/** Request ids remembered so a replayed `control_requested` is never answered twice. */
const HANDLED_REQUESTS_MAX = 512;
/** Stop records kept after they settled (for dedupe across a webview reload). */
const SETTLED_RECORDS_MAX = 16;
export const STOP_STORAGE_KEY = "praxi.stopLedger.v1";

/** Run states that are not terminal (daemon `RunState` minus `is_terminal`). */
export const OPEN_RUN_STATES: ReadonlySet<string> = new Set([
  "queued",
  "running",
  "waiting_approval",
  "paused"
]);
/** Open states in which the bot may act (`paused` stays distinct). */
export const PROCESSING_RUN_STATES: ReadonlySet<string> = new Set([
  "queued",
  "running",
  "waiting_approval"
]);

export function isOpenRunState(state: string | null | undefined): boolean {
  return state != null && OPEN_RUN_STATES.has(state);
}

export type StopOrigin = "native" | "hotkey" | "banner" | "viewer" | "chat";

/**
 * - `tracked`: live, may act;
 * - `held`: a Stop targets it and no daemon answer has confirmed anything yet;
 * - `cancel_requested`: the daemon fired its cancel token, effects may still run;
 * - `failed`: the daemon could not cancel it (retried; still never acts);
 * - `stopped`: terminal (confirmed by the daemon, `run_finished` or the run list).
 */
export type RunPhase = "tracked" | "held" | "cancel_requested" | "failed" | "stopped";

export interface RunEntry {
  readonly id: string;
  botId: string | null;
  phase: RunPhase;
  /** Latest Stop that holds this run. */
  stopId: string | null;
  /** Observed while a sweeping Stop had no answer yet: may not act until it has one. */
  quarantined: boolean;
  /** Daemon `processing` state (queued/running/waiting_approval), when known. */
  processing: boolean;
  error: string | null;
}

/**
 * - `queued`: no daemon client yet;
 * - `in_flight`: an attempt is outstanding;
 * - `retrying`: waiting for the next attempt (transport error, `failed` or still-live members);
 * - `settled`: every run it targeted is terminal;
 * - `expired`: the daemon no longer knows this id; a successor (new id, `listed`) carries on;
 * - `unconfirmable`: restored from a previous app run before the daemon ever answered and
 *   it knew no runs: the app cannot tell what it targeted, so it asks the user to press Stop again.
 */
export type StopPhase =
  | "queued"
  | "in_flight"
  | "retrying"
  | "settled"
  | "expired"
  | "unconfirmable";

export interface StopRecord {
  readonly id: string;
  origin: StopOrigin;
  /** `all`: a user Stop of everything; `single`: one run cancelled from the chat. */
  readonly kind: "all" | "single";
  /** Native stop sequences this decision accounts for (acked once it settles). */
  nativeSeqs: number[];
  /** `stop_bot_run(token = id)` was invoked and its `stopSeq` has not been accounted yet. */
  awaitingEcho: boolean;
  /** K: runs the ledger held as live when the Stop was decided (plus later explicit adds). */
  known: string[];
  /** Every run this Stop must see terminal: K, then T from the daemon. */
  members: string[];
  syncedAtDecision: boolean;
  decidedMono: number;
  scope: StopScope;
  phase: StopPhase;
  /** The daemon answered at least once (its target set is fixed). */
  reached: boolean;
  firstSeen: boolean | null;
  /** `false` when the daemon could not write the audit row of an attempt. */
  audited: boolean;
  attempts: number;
  retryIndex: number;
  lastError: string | null;
  supersedes: string | null;
  successor: string | null;
  /** Session that created it (native sequences only mean something within one). */
  session: string;
  /** Mono time it settled, for a short "stopped" confirmation. */
  settledMono: number | null;
}

export interface StopTransport {
  stopRuns(body: StopRunsInput, signal: AbortSignal): Promise<StopRunsResponse>;
}

/** The part of a native control snapshot the ledger reads. */
export interface NativeStopSnapshot {
  stateRev?: number;
  stopSeq?: number;
  lastStopToken?: string | null;
  stopAckedSeq?: number;
}

export interface NativeStopPort {
  stopRun(token: string): Promise<NativeStopSnapshot>;
  ackStop(seq: number): Promise<unknown>;
}

export interface LedgerClock {
  /** Monotonic milliseconds (`performance.now()`), only ever compared with itself. */
  now(): number;
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

export interface LedgerStorage {
  load(): string | null;
  save(value: string): void;
}

export interface LedgerDeps {
  newId(): string;
  clock: LedgerClock;
  native: NativeStopPort;
  /** Identifies this webview session (survives a reload, not an app restart). */
  session: string;
  storage?: LedgerStorage;
}

/** One daemon client as seen by the ledger. Revoked synchronously on detach. */
export interface ClientHandle {
  readonly epoch: number;
  readonly transport: StopTransport;
  live: boolean;
  /** A run list issued by THIS client has been applied and no gap happened since. */
  synced: boolean;
  /** Runs seen starting / finishing while this client's list request was in flight. */
  listing: { started: Set<string>; finished: Set<string> } | null;
}

export interface ListedRun {
  id: string;
  state: string;
  bot_id?: string | null;
}

interface Persisted {
  v: 1;
  session: string;
  records: StopRecord[];
  runs: RunEntry[];
}

type Listener = () => void;

function outcomePhase(outcome: StopOutcome, state: string | null | undefined): RunPhase {
  switch (outcome) {
    case "cancelled":
    case "already_terminal":
    case "not_found":
      return "stopped";
    case "cancel_requested":
      // Effects have ceased only once the state is terminal.
      return state && !isOpenRunState(state) ? "stopped" : "cancel_requested";
    case "failed":
      return "failed";
  }
}

const STOP_HELD: ReadonlySet<RunPhase> = new Set(["held", "cancel_requested", "failed"]);

export function isStopHeld(phase: RunPhase): boolean {
  return STOP_HELD.has(phase);
}

export class RunLedger {
  private readonly deps: LedgerDeps;
  private readonly runs = new Map<string, RunEntry>();
  private readonly records = new Map<string, StopRecord>();
  private readonly retryTimers = new Map<string, unknown>();
  private readonly inFlight = new Map<string, number>();
  private readonly handled: string[] = [];
  private readonly handledSet = new Set<string>();
  private readonly stopListeners = new Set<(rec: StopRecord) => void>();
  private readonly listeners = new Set<Listener>();
  private current: ClientHandle | null = null;
  private epochs = 0;
  /** Highest native `stopSeq` processed; `null` until the first native snapshot. */
  private stopCursor: number | null = null;
  private ackedSeq = 0;
  private lastRev = -1;
  private version = 0;
  private saved: string | null = null;

  constructor(deps: LedgerDeps) {
    this.deps = deps;
    this.restore();
  }

  // ── observation ────────────────────────────────────────────────────────────

  /** Bumped on every change (for `useSyncExternalStore`). */
  getVersion(): number {
    return this.version;
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Called once for every NEW Stop record of kind `all` (override / UI hooks). */
  onStop(listener: (rec: StopRecord) => void): () => void {
    this.stopListeners.add(listener);
    return () => {
      this.stopListeners.delete(listener);
    };
  }

  run(id: string): Readonly<RunEntry> | undefined {
    return this.runs.get(id);
  }

  runEntries(): readonly Readonly<RunEntry>[] {
    return [...this.runs.values()];
  }

  stop(id: string): Readonly<StopRecord> | undefined {
    return this.records.get(id);
  }

  stopRecords(): readonly Readonly<StopRecord>[] {
    return [...this.records.values()];
  }

  client(): ClientHandle | null {
    return this.current;
  }

  /** Native sequence the ledger has acknowledged (or seen acknowledged). */
  acked(): number {
    return this.ackedSeq;
  }

  /** Runs that are live and allowed to act. */
  liveRunIds(): string[] {
    return [...this.runs.values()]
      .filter((r) => r.phase === "tracked")
      .map((r) => r.id);
  }

  /** True when a run may still drive native / receive `proceed`. */
  mayAct(runId: string): boolean {
    const r = this.runs.get(runId);
    return (
      r !== undefined &&
      r.phase === "tracked" &&
      !r.quarantined &&
      !this.hasUnansweredSweep()
    );
  }

  /** First caller for a `control_requested` id gets `true`; replays get `false`. */
  claimRequest(requestId: string): boolean {
    if (this.handledSet.has(requestId)) {
      return false;
    }
    this.handledSet.add(requestId);
    this.handled.push(requestId);
    if (this.handled.length > HANDLED_REQUESTS_MAX) {
      const old = this.handled.shift();
      if (old !== undefined) {
        this.handledSet.delete(old);
      }
    }
    return true;
  }

  // ── daemon clients ─────────────────────────────────────────────────────────

  /**
   * A daemon client became current. The previous one is revoked first (synchronously).
   * Unresolved Stops are sent to the new client before anything else happens on it.
   */
  attachClient(transport: StopTransport): ClientHandle {
    if (this.current) {
      this.current.live = false;
    }
    this.epochs += 1;
    const handle: ClientHandle = {
      epoch: this.epochs,
      transport,
      live: true,
      synced: false,
      listing: null
    };
    this.current = handle;
    for (const rec of this.records.values()) {
      if (this.isOpenRecord(rec)) {
        this.send(rec);
      }
    }
    this.changed();
    return handle;
  }

  detachClient(handle: ClientHandle): void {
    handle.live = false;
    if (this.current === handle) {
      this.current = null;
      this.changed();
    }
  }

  /** The client's event stream had a gap or reconnected: wait for the next list. */
  markUnsynced(handle: ClientHandle): void {
    if (handle === this.current && handle.synced) {
      handle.synced = false;
      this.changed();
    }
  }

  /** Call right before issuing `GET /v1/runs` on `handle`. */
  beginList(handle: ClientHandle): void {
    if (handle !== this.current) {
      return;
    }
    handle.listing = { started: new Set(), finished: new Set() };
  }

  /**
   * Apply `GET /v1/runs` issued by `handle`. Restores which runs are live (never
   * un-holding a stopped one) and confirms stop-held runs the daemon shows terminal.
   * A list from a client that is no longer current may only move runs toward terminal.
   */
  applyList(handle: ClientHandle, list: readonly ListedRun[]): void {
    const isCurrent = handle === this.current && handle.live;
    const listing = handle.listing;
    const listed = new Map(list.map((r) => [r.id, r]));
    for (const row of list) {
      const entry = this.runs.get(row.id);
      if (!isOpenRunState(row.state)) {
        if (entry) {
          this.markTerminal(entry);
        }
        continue;
      }
      if (!isCurrent) {
        continue;
      }
      if (listing?.finished.has(row.id)) {
        continue; // finished while the list was in flight
      }
      if (entry) {
        entry.botId = row.bot_id ?? entry.botId;
        entry.processing = PROCESSING_RUN_STATES.has(row.state);
      } else {
        this.runs.set(row.id, {
          id: row.id,
          botId: row.bot_id ?? null,
          phase: "tracked",
          stopId: null,
          quarantined: this.hasUnansweredSweep(),
          processing: PROCESSING_RUN_STATES.has(row.state),
          error: null
        });
      }
    }
    if (isCurrent) {
      for (const entry of [...this.runs.values()]) {
        if (listed.has(entry.id) || listing?.started.has(entry.id)) {
          continue;
        }
        // The daemon lists every run of the user: absent means gone.
        this.markTerminal(entry);
      }
      handle.listing = null;
      handle.synced = true;
    }
    this.settleAll();
    this.changed();
  }

  /** `run_started` (fresh) or a `control_requested` for a run: it is alive. */
  observeRun(runId: string, botId: string | null, processing = true): void {
    const handle = this.current;
    handle?.listing?.started.add(runId);
    const entry = this.runs.get(runId);
    if (entry) {
      if (botId) {
        entry.botId = botId;
      }
      if (entry.phase === "tracked") {
        entry.processing = processing;
      }
      this.changed();
      return;
    }
    this.runs.set(runId, {
      id: runId,
      botId,
      phase: "tracked",
      stopId: null,
      quarantined: this.hasUnansweredSweep(),
      processing,
      error: null
    });
    this.changed();
  }

  /** `run_finished` (any, including replayed history): the run is terminal. */
  observeRunFinished(runId: string): void {
    this.current?.listing?.finished.add(runId);
    const entry = this.runs.get(runId);
    if (entry) {
      this.markTerminal(entry);
      this.settleAll();
      this.changed();
    }
  }

  // ── Stop ───────────────────────────────────────────────────────────────────

  /**
   * One user Stop. Synchronous up to and including the native call and the daemon
   * request: nothing here waits for heartbeats, hand-backs, polls or scope.
   */
  stopAll(origin: Exclude<StopOrigin, "native" | "chat">): StopRecord {
    const now = this.deps.clock.now();
    const joined = this.coalesceTarget(now, "native");
    if (joined) {
      // The same key press already reached native (global listener) first.
      this.changed();
      return joined;
    }
    const rec = this.createRecord(origin, "all", now);
    rec.awaitingEcho = true;
    // Local input halts first; the daemon request does not wait for it.
    this.deps.native.stopRun(rec.id).then(
      (snap) => {
        this.onNativeSnapshot(snap);
        rec.awaitingEcho = false;
      },
      () => {
        rec.awaitingEcho = false;
      }
    );
    this.dispatch(rec);
    return rec;
  }

  /** Cancel one run (chat Stop button): its own decision, `listed`, no native effect. */
  cancelOne(runId: string): StopRecord {
    const now = this.deps.clock.now();
    const rec = this.blankRecord("chat", "single", now);
    rec.scope = "listed";
    const entry = this.runs.get(runId);
    if (entry && entry.phase !== "stopped") {
      this.hold(entry, rec);
    } else if (!entry) {
      this.runs.set(runId, {
        id: runId,
        botId: null,
        phase: "held",
        stopId: rec.id,
        quarantined: false,
        processing: false,
        error: null
      });
    }
    rec.known = [runId];
    rec.members = [runId];
    this.records.set(rec.id, rec);
    this.dispatch(rec);
    return rec;
  }

  /**
   * Every native control snapshot (event, command reply, initial read) goes here.
   * Returns `true` when its mode is newer than the last one applied (`stateRev`).
   */
  onNativeSnapshot(snap: NativeStopSnapshot): boolean {
    if (snap.stopSeq === undefined) {
      // A failed read (or a native build without stop sequences): it says nothing about Stops.
      return this.freshRev(snap);
    }
    const s = snap.stopSeq;
    if (snap.stopAckedSeq !== undefined && snap.stopAckedSeq > this.ackedSeq) {
      this.ackedSeq = snap.stopAckedSeq;
    }
    if (this.stopCursor === null) {
      this.adoptInitial(snap);
    } else if (s > this.stopCursor) {
      const seqs: number[] = [];
      for (let q = this.stopCursor + 1; q <= s; q += 1) {
        seqs.push(q);
      }
      this.stopCursor = s;
      const token = snap.lastStopToken ?? null;
      const tokened = token ? this.records.get(token) : undefined;
      if (tokened && tokened.awaitingEcho) {
        tokened.awaitingEcho = false;
        tokened.nativeSeqs.push(s);
        seqs.pop();
      }
      // Our own invokes whose echo has not been seen account for the lowest seqs.
      for (const rec of this.records.values()) {
        if (!seqs.length) {
          break;
        }
        if (rec.awaitingEcho) {
          rec.awaitingEcho = false;
          rec.nativeSeqs.push(seqs.shift() as number);
        }
      }
      if (seqs.length) {
        this.nativePress(seqs);
      }
      this.changed();
    }
    const fresh = this.freshRev(snap);
    this.tryAck();
    return fresh;
  }

  /** Native modes are applied only from a snapshot newer than the last applied one. */
  private freshRev(snap: NativeStopSnapshot): boolean {
    if (snap.stateRev === undefined) {
      return true;
    }
    if (snap.stateRev > this.lastRev) {
      this.lastRev = snap.stateRev;
      return true;
    }
    return false;
  }

  // ── internals ──────────────────────────────────────────────────────────────

  private adoptInitial(snap: NativeStopSnapshot): void {
    const s = snap.stopSeq ?? 0;
    // Older native builds report no ack cursor: treat everything as handled.
    const base = snap.stopAckedSeq ?? s;
    this.ackedSeq = Math.max(this.ackedSeq, base);
    this.stopCursor = s;
    if (s <= base) {
      return;
    }
    const claimed = new Set<number>();
    for (const rec of this.records.values()) {
      for (const q of rec.nativeSeqs) {
        claimed.add(q);
      }
    }
    const unclaimed: number[] = [];
    for (let q = base + 1; q <= s; q += 1) {
      if (!claimed.has(q)) {
        unclaimed.push(q);
      }
    }
    // A press nobody handled yet (e.g. while this webview was loading).
    if (unclaimed.length) {
      this.nativePress(unclaimed);
    }
    this.changed();
  }

  private nativePress(seqs: number[]): void {
    const now = this.deps.clock.now();
    // The in-window hotkey and the global listener on the same press, or key bounce.
    const joined = this.coalesceTarget(now, "any");
    if (joined) {
      joined.nativeSeqs.push(...seqs);
      this.tryAck();
      return;
    }
    const rec = this.createRecord("native", "all", now);
    rec.nativeSeqs.push(...seqs);
    this.dispatch(rec);
  }

  /** A Stop decided within the coalesce window and not answered yet (same key press). */
  private coalesceTarget(now: number, want: "any" | "native"): StopRecord | null {
    let best: StopRecord | null = null;
    for (const rec of this.records.values()) {
      if (rec.kind !== "all" || rec.reached || !this.isOpenRecord(rec)) {
        continue;
      }
      if (want === "native" && rec.origin !== "native") {
        continue;
      }
      if (now - rec.decidedMono <= STOP_COALESCE_MS) {
        best = rec;
      }
    }
    return best;
  }

  private blankRecord(origin: StopOrigin, kind: "all" | "single", now: number): StopRecord {
    return {
      id: this.deps.newId(),
      origin,
      kind,
      nativeSeqs: [],
      awaitingEcho: false,
      known: [],
      members: [],
      syncedAtDecision: this.current?.synced ?? false,
      decidedMono: now,
      scope: "all_existing",
      phase: "queued",
      reached: false,
      firstSeen: null,
      audited: true,
      attempts: 0,
      retryIndex: 0,
      lastError: null,
      supersedes: null,
      successor: null,
      session: this.deps.session,
      settledMono: null
    };
  }

  private createRecord(origin: StopOrigin, kind: "all", now: number): StopRecord {
    const rec = this.blankRecord(origin, kind, now);
    for (const entry of this.runs.values()) {
      if (entry.phase !== "stopped") {
        this.hold(entry, rec);
      }
    }
    rec.known = [...rec.members];
    this.records.set(rec.id, rec);
    return rec;
  }

  private hold(entry: RunEntry, rec: StopRecord): void {
    if (entry.phase === "tracked" || entry.phase === "failed") {
      entry.phase = "held";
    }
    entry.stopId = rec.id;
    if (!rec.members.includes(entry.id)) {
      rec.members.push(entry.id);
    }
  }

  /** Send a new record, retry every other unresolved one, notify. */
  private dispatch(rec: StopRecord): void {
    this.send(rec);
    for (const other of this.records.values()) {
      if (other !== rec && this.isOpenRecord(other)) {
        this.send(other, true);
      }
    }
    if (rec.kind === "all") {
      for (const l of this.stopListeners) {
        l(rec);
      }
    }
    this.persist();
    this.changed();
  }

  private isOpenRecord(rec: StopRecord): boolean {
    return (
      rec.phase === "queued" ||
      rec.phase === "in_flight" ||
      rec.phase === "retrying"
    );
  }

  private hasUnansweredSweep(): boolean {
    for (const rec of this.records.values()) {
      if (rec.scope === "all_existing" && !rec.reached && this.isOpenRecord(rec)) {
        return true;
      }
    }
    return false;
  }

  private send(rec: StopRecord, now = false): void {
    const handle = this.current;
    if (!handle || !handle.live || !this.isOpenRecord(rec)) {
      return;
    }
    if (!now && this.inFlight.get(rec.id) === handle.epoch) {
      return; // one attempt per client at a time, unless the user pressed Stop again
    }
    if (now) {
      const timer = this.retryTimers.get(rec.id);
      if (timer !== undefined) {
        this.deps.clock.clear(timer);
        this.retryTimers.delete(rec.id);
      }
    }
    const clock = this.deps.clock;
    if (
      !rec.reached &&
      rec.scope === "all_existing" &&
      rec.syncedAtDecision &&
      clock.now() - rec.decidedMono > STOP_SWEEP_WINDOW_MS
    ) {
      // Late first delivery while the app knew the runs: never sweep a later run.
      rec.scope = "listed";
    }
    const body: StopRunsInput = {
      stop_id: rec.id,
      scope: rec.scope,
      run_ids: rec.known.slice(0, 1024)
    };
    const controller = new AbortController();
    const timeout = clock.set(() => controller.abort(), STOP_ATTEMPT_TIMEOUT_MS);
    this.inFlight.set(rec.id, handle.epoch);
    rec.phase = "in_flight";
    rec.attempts += 1;
    let reply: Promise<StopRunsResponse>;
    try {
      reply = handle.transport.stopRuns(body, controller.signal);
    } catch (err) {
      reply = Promise.reject(err);
    }
    reply.then(
      (resp) => {
        clock.clear(timeout);
        if (this.inFlight.get(rec.id) === handle.epoch) {
          this.inFlight.delete(rec.id);
        }
        this.applyReply(rec, resp);
      },
      (err: unknown) => {
        clock.clear(timeout);
        if (this.inFlight.get(rec.id) === handle.epoch) {
          this.inFlight.delete(rec.id);
        }
        if (!this.isOpenRecord(rec)) {
          return;
        }
        rec.lastError = err instanceof Error ? err.message : String(err);
        this.scheduleRetry(rec);
        this.changed();
      }
    );
  }

  private scheduleRetry(rec: StopRecord): void {
    if (!this.isOpenRecord(rec)) {
      return;
    }
    rec.phase = "retrying";
    if (this.retryTimers.has(rec.id)) {
      return;
    }
    const delay = STOP_RETRY_MS[Math.min(rec.retryIndex, STOP_RETRY_MS.length - 1)] ?? 5000;
    rec.retryIndex += 1;
    const timer = this.deps.clock.set(() => {
      this.retryTimers.delete(rec.id);
      this.send(rec);
    }, delay);
    this.retryTimers.set(rec.id, timer);
  }

  private applyReply(rec: StopRecord, resp: StopRunsResponse): void {
    if (rec.phase === "expired" || rec.phase === "settled") {
      // A late duplicate: it may still confirm terminal runs.
      for (const r of resp.results ?? []) {
        if (outcomePhase(r.outcome, r.state) === "stopped") {
          const entry = this.runs.get(r.run_id);
          if (entry) {
            this.markTerminal(entry);
          }
        }
      }
      this.settleAll();
      this.changed();
      return;
    }
    if (resp.audited === false) {
      rec.audited = false;
    }
    if (resp.expired) {
      this.expire(rec);
      return;
    }
    rec.reached = true;
    rec.lastError = null;
    if (rec.firstSeen === null) {
      rec.firstSeen = resp.first_seen;
    }
    for (const r of resp.results) {
      const phase = outcomePhase(r.outcome, r.state);
      if (!rec.members.includes(r.run_id)) {
        rec.members.push(r.run_id);
      }
      let entry = this.runs.get(r.run_id);
      if (!entry) {
        entry = {
          id: r.run_id,
          botId: null,
          phase,
          stopId: rec.id,
          quarantined: false,
          processing: false,
          error: null
        };
        this.runs.set(r.run_id, entry);
      }
      if (entry.phase === "stopped") {
        continue; // terminal is final
      }
      entry.stopId = entry.stopId ?? rec.id;
      if (entry.phase === "tracked") {
        entry.stopId = rec.id;
      }
      entry.phase = phase;
      entry.quarantined = false;
      entry.error = phase === "failed" ? (r.error ?? null) : null;
    }
    if (!this.hasUnansweredSweep()) {
      for (const entry of this.runs.values()) {
        entry.quarantined = false; // not in any target set: free to act again
      }
    }
    this.settleAll();
    if (this.isOpenRecord(rec)) {
      this.scheduleRetry(rec);
    }
    this.persist();
    this.changed();
  }

  /**
   * The daemon forgot this id. Never re-sweep: a successor with a NEW id carries
   * only the runs the app still holds, as `listed`.
   */
  private expire(rec: StopRecord): void {
    rec.phase = "expired";
    rec.settledMono = this.deps.clock.now();
    const held = rec.members.filter((id) => {
      const entry = this.runs.get(id);
      return entry !== undefined && entry.phase !== "stopped";
    });
    if (held.length) {
      const next = this.blankRecord(rec.origin, rec.kind, this.deps.clock.now());
      next.scope = "listed";
      next.known = [...held];
      next.members = [...held];
      next.supersedes = rec.id;
      next.nativeSeqs = rec.nativeSeqs;
      rec.nativeSeqs = [];
      rec.successor = next.id;
      for (const id of held) {
        const entry = this.runs.get(id);
        if (entry) {
          entry.stopId = next.id;
        }
      }
      this.records.set(next.id, next);
      this.send(next);
    }
    this.tryAck();
    this.persist();
    this.changed();
  }

  private markTerminal(entry: RunEntry): void {
    if (entry.phase === "tracked" && entry.stopId === null) {
      this.runs.delete(entry.id);
      return;
    }
    entry.phase = "stopped";
    entry.quarantined = false;
    entry.error = null;
  }

  private settleAll(): void {
    let any = false;
    for (const rec of this.records.values()) {
      if (!this.isOpenRecord(rec) || !rec.reached) {
        continue;
      }
      const open = rec.members.some((id) => {
        const entry = this.runs.get(id);
        return entry !== undefined && entry.phase !== "stopped";
      });
      if (!open) {
        rec.phase = "settled";
        rec.settledMono = this.deps.clock.now();
        const timer = this.retryTimers.get(rec.id);
        if (timer !== undefined) {
          this.deps.clock.clear(timer);
          this.retryTimers.delete(rec.id);
        }
        any = true;
      }
    }
    if (any) {
      this.prune();
      this.tryAck();
      this.persist();
    }
  }

  /** Drop stopped runs and old records nothing open refers to any more. */
  private prune(): void {
    const referenced = new Set<string>();
    for (const rec of this.records.values()) {
      if (this.isOpenRecord(rec)) {
        for (const id of rec.members) {
          referenced.add(id);
        }
      }
    }
    for (const entry of [...this.runs.values()]) {
      if (entry.phase === "stopped" && !referenced.has(entry.id)) {
        this.runs.delete(entry.id);
      }
    }
    const done = [...this.records.values()].filter((r) => !this.isOpenRecord(r));
    const excess = done.length - SETTLED_RECORDS_MAX;
    for (let i = 0; i < excess; i += 1) {
      const rec = done[i];
      if (rec) {
        this.records.delete(rec.id);
      }
    }
  }

  /**
   * Acknowledge the highest native sequence below every unresolved Stop. A Stop is
   * unresolved until every run it targeted is terminal (expired ones pass their
   * sequences to the successor).
   */
  private tryAck(): void {
    if (this.stopCursor === null) {
      return;
    }
    let target = this.stopCursor;
    for (const rec of this.records.values()) {
      if (rec.phase === "settled" || rec.phase === "expired") {
        continue;
      }
      for (const q of rec.nativeSeqs) {
        target = Math.min(target, q - 1);
      }
    }
    if (target > this.ackedSeq) {
      this.ackedSeq = target;
      void this.deps.native.ackStop(target).catch(() => undefined);
    }
  }

  private persist(): void {
    const storage = this.deps.storage;
    if (!storage) {
      return;
    }
    const data: Persisted = {
      v: 1,
      session: this.deps.session,
      records: [...this.records.values()],
      runs: [...this.runs.values()].filter((r) => r.stopId !== null)
    };
    try {
      const json = JSON.stringify(data);
      if (json !== this.saved) {
        this.saved = json;
        storage.save(json);
      }
    } catch {
      // storage unavailable: Stop still works in memory
    }
  }

  /**
   * Stop records survive a webview reload and an app restart. Native sequences are
   * only meaningful within the session that saw them; a Stop from an earlier app run
   * that never reached the daemon is narrowed to the runs it knew (never a sweep).
   */
  private restore(): void {
    const storage = this.deps.storage;
    if (!storage) {
      return;
    }
    let data: Persisted | null = null;
    try {
      const raw = storage.load();
      data = raw ? (JSON.parse(raw) as Persisted) : null;
    } catch {
      data = null;
    }
    if (!data || data.v !== 1 || !Array.isArray(data.records)) {
      return;
    }
    const sameSession = data.session === this.deps.session;
    for (const entry of data.runs ?? []) {
      this.runs.set(entry.id, { ...entry, quarantined: false });
    }
    for (const saved of data.records) {
      const rec: StopRecord = {
        ...saved,
        awaitingEcho: false,
        // mono clocks restart with the page
        decidedMono: Number.NEGATIVE_INFINITY,
        settledMono: saved.settledMono === null ? null : Number.NEGATIVE_INFINITY
      };
      if (!sameSession) {
        rec.nativeSeqs = [];
        if (this.isOpenRecord(rec) && !rec.reached && rec.scope === "all_existing") {
          rec.scope = "listed";
          if (rec.known.length === 0) {
            rec.phase = "unconfirmable";
          }
        }
      }
      if (this.isOpenRecord(rec)) {
        rec.phase = "queued";
      }
      this.records.set(rec.id, rec);
    }
  }

  private changed(): void {
    this.version += 1;
    this.persist();
    for (const l of this.listeners) {
      l();
    }
  }
}
