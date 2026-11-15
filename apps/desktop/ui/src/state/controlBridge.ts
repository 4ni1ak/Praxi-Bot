import type {
  ControlModeName,
  ControlStateDto,
  ControlVerdict,
  DaemonEvent,
  GlobalEnvelope,
  Run,
  ReleaseRequest,
  TakeoverInfo,
  TargetInfo,
  ViewReason
} from "../api/types.ts";
import { isCancellableRunState, isProcessingRunState } from "../lib/runState.ts";

/**
 * The app side of the daemon <-> app control bridge.
 *
 * Bot desktop actions run inside the daemon; the native overlay (local desktop
 * targets only) and panel overlay (browser / remote targets) mirror and gate
 * them here:
 *
 * - `control_requested` events:
 *   - known local desktop target -> `native.beginAction` + `native.click` + verdict;
 *   - browser / remote target -> panel-local overlay highlight + verdict
 *     (remote screen coordinates NEVER touch local desktop monitors or mouse);
 * - run lifecycle: a local-desktop run starts -> `bot_driving`; the last run ends -> `idle`;
 * - legacy local control changes report via `PUT /v1/control/state`; plain
 *   heartbeats and reopens NEVER send `release: true`;
 * - while a daemon-owned takeover holds the latch, only `POST /v1/control/release`
 *   ("Bitirdim"), `POST /v1/control/override` (Stop) or `POST /v1/control/recover`
 *   touch the latch — heartbeats/reopen never clear it;
 * - the ONLY native transition into `idle` is `stop_all`, which always raises
 *   `run_generation`: an idle snapshot with a higher generation is an explicit
 *   Stop (from any mode, `human_driving` included) and cancels live runs. An idle
 *   snapshot with the SAME generation (hotkey applied/edited, step text, overlay
 *   clear, refused action) is not a Stop, and neither is the echo of a finish the
 *   bridge itself caused. A hand-back (`give_back`: `human_driving -> bot_driving`)
 *   and `releaseOwnedTakeover` never cancel. `emergencyStop()` cancels too.
 */

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type TargetScope = "local_desktop" | "browser" | "remote";

export type ComputerTargetSetting = "this-pc" | "browser" | "remote" | "off";

/**
 * Determine whether a run / control_requested action targets the local PC
 * monitor (`local_desktop`) vs a browser or remote host (`browser` / `remote`).
 * Remote and browser coordinates MUST NEVER drive the local desktop overlay.
 */
export function resolveTargetScope(params: {
  runId?: string;
  botId?: string;
  targets: readonly TargetInfo[];
  selectedTarget: TargetInfo | null;
  computerTarget: ComputerTargetSetting;
}): TargetScope {
  const { runId, botId, targets, selectedTarget, computerTarget } = params;
  const matched =
    targets.find((t) => runId && t.run_id === runId) ??
    targets.find((t) => botId && t.bot_id === botId) ??
    (selectedTarget?.run_id && selectedTarget.run_id === runId
      ? selectedTarget
      : null);

  if (matched) {
    if (matched.kind === "browser") {
      return "browser";
    }
    if (
      matched.unavailable_reason === "remote_node_live_view" ||
      (matched.host_id !== "local" && computerTarget === "remote") ||
      matched.host_id.startsWith("peer:")
    ) {
      return "remote";
    }
    return "local_desktop";
  }

  if (computerTarget === "browser" || computerTarget === "off") {
    return "browser";
  }
  if (computerTarget === "remote") {
    return "remote";
  }
  return "local_desktop";
}

export interface PanelActionHighlight {
  runId: string;
  botId: string;
  requestId: string;
  rect: Rect | null;
  label: string;
  action: string;
  timestampMs: number;
}

export interface BotMeta {
  botId: string;
  botName: string;
  botShape?: string;
  botAvatar?: string;
  botColor?: string;
  botInitials?: string;
}

export interface BridgeDeps {
  client: {
    listRuns(): Promise<(Pick<Run, "id" | "bot_id" | "state"> & { created_at_ms?: number })[]>;
    putControlState(mode: ControlModeName, release?: boolean): Promise<unknown>;
    decideControl(
      requestId: string,
      decision: ControlVerdict,
      reason?: string
    ): Promise<unknown>;
    cancelRun(runId: string): Promise<unknown>;
    getControlState?(): Promise<ControlStateDto>;
    releaseControl?(req?: ReleaseRequest): Promise<ControlStateDto>;
    overrideControl?(): Promise<ControlStateDto>;
    recoverControl?(): Promise<ControlStateDto>;
  };
  native: {
    beginAction(rect: Rect | null, label: string, action: string): Promise<boolean>;
    startRun(bot: BotMeta, step: string): Promise<void>;
    /** Returns the generation emitted by this programmatic native idle transition. */
    stopRun(): Promise<number>;
    click(x: number, y: number): Promise<void>;
  };
  /** Resolve whether a run / bot currently targets the local PC or a browser/remote screen. */
  resolveTargetScope?: (info: { runId?: string; botId?: string }) => TargetScope;
  /**
   * False until the run targets AND the computer-target setting have been
   * fetched (settled): a restored run's scope is never guessed before that.
   */
  targetsLoaded?(): boolean;
  /** Shared across bridge replacements: what every Stop decided so far targets. */
  ledger?: StopLedger;
  /** How long a control request waits for the run's scope to become known (ms). */
  scopeWaitMs?: number;
  /** A heartbeat that does not answer within this many ms no longer blocks the chain. */
  heartbeatBoundMs?: number;
  /** A cancel that does not settle within this many ms no longer blocks the chain. */
  cancelBoundMs?: number;
  /** Bounded wait (injectable for tests; defaults to a timer). */
  wait?: (p: Promise<unknown>, ms: number) => Promise<void>;
  /** Optional hook to render non-local action highlights inside the live-view panel. */
  panelOverlay?: {
    showAction(highlight: PanelActionHighlight): void;
  };
  /** Every control snapshot the bridge applied (the app's single source of truth). */
  onSnapshot?(dto: ControlStateDto, ticket?: number): void;
  /** Ticket drawn when a snapshot GET is issued (orders it against user actions). */
  issueSnapshotTicket?(): number;
  botMeta(botId: string): BotMeta;
  startingStep: string;
  now(): number;
}

export interface TrackedTakeover {
  id?: string;
  state: string;
  owner: string;
  targetId: string;
  reason?: ViewReason | null;
}

/** Heartbeat period; the daemon expires a registration after 15 s. */
export const HEARTBEAT_MS = 5000;
/** A `control_requested` older than this was already given up on by the daemon. */
export const STALE_REQUEST_MS = 15_000;
/** Events older than the bridge (a replay of history) are ignored, minus skew. */
const REPLAY_SLACK_MS = 2000;
/** Longest the bridge chain waits for one heartbeat (a hung daemon must not block a Stop). */
const HEARTBEAT_BOUND_MS = 2000;
/** Longest the chain waits for the cancels of one Stop to settle. */
const CANCEL_BOUND_MS = 3000;
/** Longest a control request waits for its run's scope to become known. */
const SCOPE_WAIT_MS = 5000;

/** A native `control:state_changed` as the host sees it. */
export interface NativeEvent {
  mode: ControlModeName;
  generation: number;
  /** When the event reached the app (a Stop's cutoff for I1). */
  at?: number;
}

/**
 * Judge native events buffered behind the initial snapshot.
 *  - the generation walk starts at the LOWEST generation seen, so an idle the
 *    snapshot already contains is still recognised as a Stop;
 *  - an event older than the snapshot never changes the mode, BUT an idle among
 *    them that was a generation rise is a Stop whose runs still have to be
 *    cancelled (`historicStops`): superseded only in mode, not in effect.
 */
export function planNativeReplay(
  snapshot: NativeEvent,
  buffered: readonly NativeEvent[],
  /** Highest generation received before the buffer started (a previous session's). */
  floor: number | null = null
): { baseline: number; replay: NativeEvent[]; historicStops: NativeEvent[] } {
  const lowest = buffered.reduce(
    (min, ev) => Math.min(min, ev.generation),
    floor === null ? snapshot.generation : Math.min(snapshot.generation, floor)
  );
  const historicStops: NativeEvent[] = [];
  let high = lowest;
  for (const ev of buffered) {
    if (ev.generation < snapshot.generation && ev.mode === "idle" && ev.generation > high) {
      historicStops.push(ev);
    }
    high = Math.max(high, ev.generation);
  }
  return {
    baseline: lowest,
    replay: buffered.filter((ev) => ev.generation >= snapshot.generation),
    historicStops
  };
}

/** Wait for `p`, but never longer than `ms` (a hung retired client must not block the next bridge). */
export function waitBounded(p: Promise<unknown>, ms: number): Promise<void> {
  return new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, ms);
    p.then(
      () => {
        clearTimeout(timer);
        resolve();
      },
      () => {
        clearTimeout(timer);
        resolve();
      }
    );
  });
}

/**
 * Shared by every bridge of one host (a bridge is replaced when the daemon
 * client changes). It is what makes a Stop outlive the bridge that decided it.
 *
 * Invariants the bridge enforces through it:
 *  I1 a Stop applies to exactly the runs that existed when it was decided
 *     (tracked ids, plus runs a pending restore finds that were created at or
 *     before `pendingCutoff`), never to a run created later;
 *  I3 after a Stop every targeted run is either confirmed cancelled (`done`) or
 *     tracked-and-cancellable by the CURRENT bridge: a cancel is sent before any
 *     other await, and its failure re-tracks the run on whichever bridge is current.
 */
export class StopLedger {
  /** Runs whose cancel succeeded (until the daemon stops listing them). */
  readonly done = new Set<string>();
  /** Cutoff (ms) of the newest Stop whose restore sweep has not run yet. */
  pendingCutoff: number | null = null;
  /** Highest native `run_generation` any session RECEIVED (baseline for the next session). */
  lastGeneration: number | null = null;
  private current: ControlBridge | null = null;
  /**
   * Generations that a bridge-initiated native stop (a finished run) caused: the
   * echo event may reach a DIFFERENT bridge than the one whose invoke returns.
   */
  readonly autoGenerations = new Set<number>();
  private autoPending = 0;
  private autoWaiters: (() => void)[] = [];

  beginAutoStop(): void {
    this.autoPending += 1;
  }

  endAutoStop(generation?: number): void {
    if (generation !== undefined) this.autoGenerations.add(generation);
    this.autoPending = Math.max(0, this.autoPending - 1);
    if (this.autoPending === 0) {
      for (const w of this.autoWaiters.splice(0)) w();
    }
  }

  /** Resolves when no bridge-initiated native stop is still waiting for its generation. */
  whenAutoSettled(): Promise<void> {
    if (this.autoPending === 0) return Promise.resolve();
    return new Promise<void>((resolve) => this.autoWaiters.push(resolve));
  }

  /**
   * A Stop was decided before its deciding bridge swept the daemon's run list.
   * If the current bridge already swept, it applies the Stop to its tracked runs;
   * otherwise the cutoff waits for whichever bridge sweeps.
   */
  noteStop(cutoff: number, origin: ControlBridge): void {
    const cur = this.current;
    if (cur && cur !== origin && cur.hasRestored()) {
      cur.applyStopTo(cutoff);
      return;
    }
    this.pendingCutoff = Math.max(this.pendingCutoff ?? cutoff, cutoff);
  }

  attach(bridge: ControlBridge): void {
    this.current = bridge;
  }

  detach(bridge: ControlBridge): void {
    if (this.current === bridge) this.current = null;
  }

  settle(id: string, ok: boolean): void {
    if (ok) {
      this.done.add(id);
      this.current?.untrack(id);
    } else {
      this.current?.track(id);
    }
  }

  prune(listed: ReadonlySet<string>): void {
    for (const id of [...this.done]) {
      if (!listed.has(id)) this.done.delete(id);
    }
  }
}

export function refusalReason(mode: ControlModeName): string {
  switch (mode) {
    case "human_driving":
      return "The user has taken control of the desktop, so this action was not performed. Do not retry it; ask the user to give control back.";
    case "paused":
      return "The user has paused desktop control for this bot, so this action was not performed. Do not retry it; ask the user to resume.";
    default:
      return "The user stopped this desktop action, so this action was not performed. Do not retry it; ask the user how to continue.";
  }
}

function centre(r: Rect): { x: number; y: number } {
  return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
}

export class ControlBridge {
  mode: ControlModeName = "idle";
  private readonly active = new Set<string>();
  /** When each tracked run was created/first seen (ms): a Stop targets only older runs (I1). */
  private readonly startedAt_ = new Map<string, number>();
  private readonly handled = new Set<string>();
  private readonly startedAt: number;
  private readonly deps: BridgeDeps;
  private chain: Promise<void> = Promise.resolve();
  private disposed = false;
  private restored = false;
  private restoring: Promise<void> | null = null;
  private snapshot: { finished: Set<string> } | null = null;
  /** Cutoff (ms) of a Stop decided before the restore sweep ran; null = none. */
  private stopCutoff: number | null = null;
  private automaticIdleGeneration: number | null = null;
  /** Highest native `run_generation` seen (initial snapshot or event). */
  private nativeGeneration: number | null = null;
  /** Runs tracked when the bridge was retired (for Stops queued before that). */
  private retired: string[] = [];
  private deferredStarts: { botId: string; runId: string }[] = [];
  private activeTakeover: TrackedTakeover | null = null;
  private releasingTakeover = false;
  /** Id of the takeover THIS app owns (set by the live-view controller). */
  private ownedTakeoverId: string | null = null;

  constructor(deps: BridgeDeps) {
    this.deps = deps;
    this.startedAt = deps.now();
    deps.ledger?.attach(this);
  }

  private bounded(p: Promise<unknown>, ms: number): Promise<void> {
    return (this.deps.wait ?? waitBounded)(p, ms);
  }

  /** True once the restore sweep has run (a later Stop is handled by the tracked ids). */
  hasRestored(): boolean {
    return this.restored;
  }

  /** A Stop decided elsewhere (a retiring bridge) that applies to what this bridge tracks. */
  applyStopTo(cutoff: number): void {
    void this.bounded(
      this.cancelIds(this.takeRuns(cutoff)),
      this.deps.cancelBoundMs ?? CANCEL_BOUND_MS
    );
  }

  /** Track a run again (a cancel failed); never a confirmed-cancelled one. */
  track(id: string): void {
    if (!this.disposed && !this.deps.ledger?.done.has(id)) this.active.add(id);
  }

  /** Forget a run whose cancel was confirmed. */
  untrack(id: string): void {
    this.active.delete(id);
  }

  /** Run ids the bridge believes are live (for tests / diagnostics). */
  liveRuns(): string[] {
    return [...this.active];
  }

  /** Retire the old daemon connection before a replacement bridge can act. */
  dispose(): void {
    // Native events already queued on this bridge still get to cancel the runs it
    // tracked (a Stop during a daemon client replacement), see `nativeState`; any
    // event arriving after this point is ignored.
    this.retired = [...this.active];
    this.disposed = true;
    this.deps.ledger?.detach(this);
    this.active.clear();
    this.handled.clear();
    this.deferredStarts = [];
  }

  /** Resolves once everything queued so far (including cancels) has been issued. */
  drain(): Promise<void> {
    return this.chain;
  }

  /** Initial native state is a snapshot, never an explicit Stop or resume. */
  initializeNativeState(mode: ControlModeName, generation?: number): Promise<void> {
    this.chain = this.chain.then(async () => {
      if (this.disposed) return;
      if (generation !== undefined) this.noteGeneration(generation);
      if (mode !== "idle" || this.mode === "idle") this.mode = mode;
      await this.heartbeat();
    }).catch(() => undefined);
    return this.chain;
  }

  /** Subscribe to events first, then restore live runs that predate this UI. */
  restoreRuns(): Promise<void> {
    if (this.disposed || this.restored) return Promise.resolve();
    if (this.restoring) return this.restoring;
    const snapshot = { finished: new Set<string>() };
    this.snapshot = snapshot;
    this.restoring = (async () => {
      try {
        const rows = await this.deps.client.listRuns();
        this.deps.ledger?.prune(new Set(rows.map((r) => r.id)));
        this.chain = this.chain.then(async () => {
          if (this.disposed) return;
          const live = rows.filter(
            (run) =>
              isCancellableRunState(run.state) &&
              !snapshot.finished.has(run.id) &&
              !this.deps.ledger?.done.has(run.id)
          );
          // I1: a pending Stop targets only runs that existed when it was decided.
          // Read at sweep time: a Stop noted by a retiring bridge after this one was
          // created must still count.
          const pendingNow = this.deps.ledger?.pendingCutoff ?? null;
          const cutoff =
            this.stopCutoff === null
              ? pendingNow
              : pendingNow === null
                ? this.stopCutoff
                : Math.max(this.stopCutoff, pendingNow);
          const targeted =
            cutoff === null
              ? []
              : live.filter((run) => (run.created_at_ms ?? -Infinity) <= cutoff);
          const targetedIds = new Set(targeted.map((run) => run.id));
          const keep = live.filter((run) => !targetedIds.has(run.id));
          const cancelling = this.cancelIds(targeted.map((run) => run.id));
          for (const run of keep) {
            this.active.add(run.id);
            this.noteStarted(run.id, run.created_at_ms ?? -Infinity);
          }
          const processing = keep.filter((run) => isProcessingRunState(run.state));
          for (const run of processing) {
            await this.ensureDriving(run.bot_id, run.id);
          }
          if (processing.length === 0 && this.active.size === 0 && this.mode === "bot_driving") {
            await this.stopNativeRun();
          }
          await this.bounded(cancelling, this.deps.cancelBoundMs ?? CANCEL_BOUND_MS);
          this.restored = true;
          const ledger = this.deps.ledger;
          // A bridge retired mid-sweep must not consume the pending Stop: its
          // tracked set is stale; the next bridge's sweep still has to apply it.
          if (ledger && ledger.pendingCutoff !== null && !this.disposed) {
            // A Stop noted while the sweep ran (or after the list was read) applies
            // to the runs this bridge now tracks.
            const late = ledger.pendingCutoff;
            ledger.pendingCutoff = null;
            if (cutoff === null || late > cutoff) {
              await this.bounded(
                this.cancelIds(this.takeRuns(late)),
                this.deps.cancelBoundMs ?? CANCEL_BOUND_MS
              );
            }
          }
          this.stopCutoff = null;
        }).catch(() => undefined);
        await this.chain;
      } catch {
        // A failed initial read is retried by the next heartbeat.
      } finally {
        this.snapshot = null;
        this.restoring = null;
      }
    })();
    return this.restoring;
  }

  /** Currently tracked daemon takeover (owned or peer), if any. */
  takeover(): TrackedTakeover | null {
    return this.activeTakeover;
  }

  /** Tell the bridge which takeover this app owns; only that one blocks legacy release. */
  setOwnedTakeoverId(id: string | null): void {
    this.ownedTakeoverId = id;
  }

  /** Re-read `GET /v1/control/state` (events are notifications only and are not replayed). */
  refreshControlState(): Promise<void> {
    const get = this.deps.client.getControlState;
    if (!get) {
      return this.chain;
    }
    this.chain = this.chain
      .then(async () => {
        const ticket = this.deps.issueSnapshotTicket?.();
        this.applyControlDto(await get.call(this.deps.client), ticket);
      })
      .catch(() => undefined);
    return this.chain;
  }

  /** Sync authoritative daemon control snapshot into the bridge. */
  syncControlState(dto: ControlStateDto): Promise<void> {
    this.chain = this.chain
      .then(() => {
        this.applyControlDto(dto);
      })
      .catch(() => undefined);
    return this.chain;
  }

  /** Feed one daemon event; work is serialised in arrival order. */
  handleEvent(env: GlobalEnvelope): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (env.event.type === "run_finished" && this.fresh(env)) {
      this.snapshot?.finished.add(env.event.run_id);
    }
    this.chain = this.chain
      .then(() => this.process(env))
      .catch(() => undefined);
    return this.chain;
  }

  /** The native state machine changed (Tauri `control:state_changed`). */
  onNativeState(mode: ControlModeName, generation?: number, at?: number): Promise<void> {
    if (this.disposed) return Promise.resolve();
    // The moment the event reached the app, not the moment the chain gets to it:
    // runs created in between are NOT targeted by a Stop (I1).
    const receivedAt = at ?? this.deps.now();
    // Queued while this bridge was still current: if it is retired before the
    // event runs, a Stop must still cancel what the bridge tracked.
    this.chain = this.chain
      .then(() => this.nativeState(mode, generation, true, receivedAt))
      .catch(() => undefined);
    return this.chain;
  }

  /**
   * A Stop that happened before the initial snapshot (so it never changes the
   * mode) but whose effect still applies to the runs that existed then.
   */
  applyHistoricStop(generation: number, at: number): Promise<void> {
    if (this.disposed) {
      this.deps.ledger?.noteStop(at, this);
      return Promise.resolve();
    }
    this.chain = this.chain
      .then(async () => {
        if (this.deps.ledger) {
          await this.bounded(
            this.deps.ledger.whenAutoSettled(),
            this.deps.heartbeatBoundMs ?? HEARTBEAT_BOUND_MS
          );
          if (this.deps.ledger.autoGenerations.has(generation)) return; // our own finish
        }
        if (generation === this.automaticIdleGeneration) return;
        this.markStopRequested(at);
        await this.bounded(
          this.cancelIds(this.takeRuns(at)),
          this.deps.cancelBoundMs ?? CANCEL_BOUND_MS
        );
      })
      .catch(() => undefined);
    return this.chain;
  }

  /**
   * Explicitly hand back an owned takeover ("Bitirdim") without cancelling live
   * runs. A failed release rejects (the caller decides what the daemon state
   * means); the internal serialisation chain stays usable.
   */
  releaseOwnedTakeover(req: ReleaseRequest): Promise<ControlStateDto | null> {
    const run = this.chain.then(async () => {
      if (!this.deps.client.releaseControl) {
        return null;
      }
      this.releasingTakeover = true;
      try {
        const result = await this.deps.client.releaseControl(req);
        this.applyControlDto(result);
        return result;
      } finally {
        this.releasingTakeover = false;
      }
    });
    this.chain = run.then(
      () => undefined,
      () => undefined
    );
    return run;
  }

  /** Explicit user Stop override: pauses any active takeover and cancels live runs. */
  emergencyStop(): Promise<ControlStateDto | null> {
    let result: ControlStateDto | null = null;
    const stopAt = this.deps.now();
    this.chain = this.chain
      .then(async () => {
        // Decide which runs to cancel NOW, and send the cancels before any other
        // await (override / native stop can hang): the Stop must not be lost.
        this.markStopRequested(stopAt);
        const cancelling = this.cancelIds(this.takeRuns(stopAt));
        if (this.deps.client.overrideControl) {
          try {
            result = await this.deps.client.overrideControl();
            this.applyControlDto(result);
          } catch {
            // fall through to local stop / cancel
          }
        }
        await this.deps.native.stopRun().catch(() => undefined);
        this.mode = "idle";
        await this.bounded(
          Promise.all([cancelling, this.cancelIds(this.takeRuns(stopAt))]),
          this.deps.cancelBoundMs ?? CANCEL_BOUND_MS
        );
      })
      .catch(() => undefined);
    return this.chain.then(() => result);
  }

  /** Send the current mode (call every `HEARTBEAT_MS` and on connect). */
  async heartbeat(release = false): Promise<void> {
    if (this.disposed) return;
    // Heartbeats and reopens never release an owned takeover latch.
    const effectiveRelease = release && !this.hasOwnedTakeover();
    try {
      // Bounded: a hung PUT must not hold the chain (and with it a queued Stop).
      await this.bounded(
        this.deps.client.putControlState(this.mode, effectiveRelease || undefined),
        this.deps.heartbeatBoundMs ?? HEARTBEAT_BOUND_MS
      );
    } catch {
      // daemon unreachable or 409 because owned takeover holds the latch
    }
  }

  private applyTakeoverInfo(t: TakeoverInfo | null | undefined): void {
    if (!t) {
      this.activeTakeover = null;
      return;
    }
    this.activeTakeover = {
      id: t.id,
      state: t.state,
      owner: t.owner,
      targetId: t.target_id,
      reason: t.reason ?? null
    };
  }

  private applyControlDto(dto: ControlStateDto, ticket?: number): void {
    this.applyTakeoverInfo(dto.takeover);
    this.deps.onSnapshot?.(dto, ticket);
  }

  /** True only for this app's own live takeover, never a peer's or a stale one. */
  private hasOwnedTakeover(): boolean {
    if (this.releasingTakeover) {
      return true;
    }
    const t = this.activeTakeover;
    return (
      t !== null &&
      this.ownedTakeoverId !== null &&
      t.id === this.ownedTakeoverId &&
      (t.state === "active" || t.state === "acquiring")
    );
  }

  private scopeFor(info: { runId?: string; botId?: string }): TargetScope {
    return this.deps.resolveTargetScope?.(info) ?? "local_desktop";
  }

  /** Retired bridge: take only the runs that existed at the Stop; later Stops still see the rest. */
  private takeRetired(cutoff: number): string[] {
    const taken = this.upTo(this.retired, cutoff);
    this.retired = this.retired.filter((id) => !taken.includes(id));
    return taken;
  }

  private noteStarted(id: string, at: number): void {
    const known = this.startedAt_.get(id);
    // An authoritative time (event / daemon row) replaces an unknown one.
    if (known === undefined || (known === -Infinity && at !== -Infinity)) {
      this.startedAt_.set(id, at);
    }
  }

  /** The ids that existed at `cutoff` (everything when undefined). */
  private upTo(ids: Iterable<string>, cutoff?: number): string[] {
    return [...ids].filter(
      (id) => cutoff === undefined || (this.startedAt_.get(id) ?? -Infinity) <= cutoff
    );
  }

  /**
   * Copy and clear the tracked run ids that existed when the Stop was decided
   * (I1): a run created after `cutoff` stays tracked and untouched.
   */
  private takeRuns(cutoff?: number): string[] {
    const runs = this.upTo(this.active, cutoff);
    for (const id of runs) this.active.delete(id);
    return runs;
  }

  /**
   * Send the cancels now; each outcome settles in the shared ledger: confirmed
   * ones are never re-tracked, failed ones are tracked again on whichever bridge
   * is current (this one may already be disposed).
   */
  private cancelIds(ids: string[]): Promise<void> {
    return Promise.all(
      ids.map((id) =>
        this.deps.client.cancelRun(id).then(
          () => this.settleCancel(id, true),
          () => this.settleCancel(id, false)
        )
      )
    ).then(() => undefined);
  }

  private settleCancel(id: string, ok: boolean): void {
    if (this.deps.ledger) {
      this.deps.ledger.settle(id, ok);
    } else if (!ok && !this.disposed) {
      this.active.add(id);
    }
  }

  private noteGeneration(generation: number): void {

    if (this.nativeGeneration === null || generation > this.nativeGeneration) {
      this.nativeGeneration = generation;
    }
  }

  /**
   * An idle snapshot is a Stop only when `run_generation` rose (`stop_all`), and
   * not for the single generation `stopNativeRun` just caused. Without a
   * generation (older callers, unit tests) it is taken as a Stop.
   */
  private isStopIdle(generation: number | undefined): boolean {
    if (generation === undefined) return true;
    if (generation === this.automaticIdleGeneration) return false;
    if (this.deps.ledger?.autoGenerations.has(generation)) return false;
    return this.nativeGeneration === null || generation > this.nativeGeneration;
  }

  /** The scope of a run is known only once the targets (and settings) of THIS client settled. */
  private scopeKnown(): boolean {
    return this.deps.targetsLoaded ? this.deps.targetsLoaded() : true;
  }

  /** Wait (bounded) for the scope to become known; null = still unknown. */
  private async awaitScope(info: { runId?: string; botId?: string }): Promise<TargetScope | null> {
    const deadline = Date.now() + (this.deps.scopeWaitMs ?? SCOPE_WAIT_MS);
    while (!this.scopeKnown()) {
      if (this.disposed || Date.now() >= deadline) return null;
      await new Promise((r) => setTimeout(r, 25));
    }
    return this.scopeFor(info);
  }

  /** Retry overlays that were waiting for the scope to be known (call each tick). */
  retryDeferredStarts(): Promise<void> {
    if (this.deferredStarts.length === 0) return Promise.resolve();
    this.chain = this.chain
      .then(async () => {
        const waiting = this.deferredStarts;
        this.deferredStarts = [];
        for (const d of waiting) {
          if (this.disposed || !this.active.has(d.runId)) continue;
          await this.ensureDriving(d.botId, d.runId); // re-defers while unknown
        }
      })
      .catch(() => undefined);
    return this.chain;
  }

  private async nativeState(
    mode: ControlModeName,
    generation?: number,
    queuedWhileCurrent = false,
    receivedAt: number = this.deps.now()
  ): Promise<void> {
    if (mode === "idle" && generation !== undefined && this.deps.ledger) {
      // The echo of a stop another bridge caused may arrive before that bridge's
      // invoke returns with its generation: wait (bounded) before judging.
      await this.bounded(
        this.deps.ledger.whenAutoSettled(),
        this.deps.heartbeatBoundMs ?? HEARTBEAT_BOUND_MS
      );
    }
    const stop = mode === "idle" && this.isStopIdle(generation);
    if (generation !== undefined) this.noteGeneration(generation);
    if (this.disposed) {
      // A Stop queued on a retired bridge (the daemon client was replaced) still
      // cancels the runs it tracked; nothing else is applied.
      if (stop && queuedWhileCurrent) {
        this.markStopRequested(receivedAt);
        await this.bounded(
          this.cancelIds(this.takeRetired(receivedAt)),
          this.deps.cancelBoundMs ?? CANCEL_BOUND_MS
        );
      }
      return;
    }
    if (mode === "idle" && !stop) {
      // Same-generation idle (hotkey applied/edited, step text, overlay clear,
      // refused action) or the echo of our own finish: not a Stop.
      await this.heartbeat();
      return;
    }
    const previous = this.mode;
    this.mode = mode;
    let cancelling: Promise<void> | null = null;
    if (stop) {
      // Decide now and send the cancels before any other await: a Stop must
      // survive a bridge swap and a hung heartbeat, and it also covers runs a
      // pending restore has not found yet (see StopLedger, I1/I3).
      this.markStopRequested(receivedAt);
      cancelling = this.cancelIds(this.takeRuns(receivedAt));
    }

    // While an owned takeover controls the daemon latch, a local Pause overrides
    // instead of fighting the latch, and leaving human_driving never sends legacy release:true.
    if (mode === "paused" && this.hasOwnedTakeover() && this.deps.client.overrideControl) {
      try {
        const dto = await this.deps.client.overrideControl();
        this.applyControlDto(dto);
      } catch {
        // next beat / state check retries
      }
    } else {
      const resumed =
        !this.hasOwnedTakeover() &&
        (previous === "human_driving" || previous === "paused") &&
        (mode === "idle" || mode === "bot_driving");
      await this.heartbeat(resumed);
    }

    if (cancelling) {
      await this.bounded(cancelling, this.deps.cancelBoundMs ?? CANCEL_BOUND_MS);
    }
  }

  private fresh(env: GlobalEnvelope): boolean {
    if (env.event.type === "control_state_changed") {
      return true;
    }
    return env.ts_ms >= this.startedAt - REPLAY_SLACK_MS;
  }

  private async stopNativeRun(): Promise<void> {
    this.deps.ledger?.beginAutoStop();
    let generation: number | undefined;
    try {
      generation = await this.deps.native.stopRun();
      this.automaticIdleGeneration = generation;
    } finally {
      this.deps.ledger?.endAutoStop(generation);
    }
    if (!this.disposed) this.mode = "idle";
  }

  private markStopRequested(at?: number): void {
    const cutoff = at ?? this.deps.now();
    this.stopCutoff = Math.max(this.stopCutoff ?? cutoff, cutoff);
    // A Stop decided before the restore sweep ran is carried to whichever bridge
    // sweeps; one decided after it is fully handled by the tracked ids.
    // A retired bridge's tracked set is stale (events queued behind a hung step
    // never reached it), so its Stop always goes through the ledger to whichever
    // bridge is current; so does one decided before the sweep.
    if (!this.restored || this.disposed) this.deps.ledger?.noteStop(cutoff, this);
    for (const id of this.active) this.snapshot?.finished.add(id);
  }

  private async ensureDriving(botId: string, runId?: string): Promise<void> {
    if (this.disposed) return;
    if (!this.scopeKnown()) {
      // I2: never start the local overlay for a run whose scope is unknown.
      if (runId && !this.deferredStarts.some((d) => d.runId === runId)) {
        this.deferredStarts.push({ botId, runId });
      }
      return;
    }
    if (this.scopeFor({ botId, runId }) !== "local_desktop") {
      return;
    }
    if (this.mode === "idle") {
      await this.deps.native.startRun(
        this.deps.botMeta(botId),
        this.deps.startingStep
      );
      // The state machine reports bot_driving through `onNativeState`; do not
      // wait for it to decide this request.
      if (!this.disposed) this.mode = "bot_driving";
    }
  }

  private async process(env: GlobalEnvelope): Promise<void> {
    if (this.disposed) return;
    const e: DaemonEvent = env.event;
    if (!this.fresh(env)) {
      return;
    }
    switch (e.type) {
      case "control_state_changed":
        if (e.state === "released" || e.state === "recovered") {
          this.activeTakeover = null;
        } else {
          // The event never carries a takeover id: keep a known id for the same
          // actor/target and let the snapshot below fill in the rest.
          const prev = this.activeTakeover;
          const same =
            prev !== null && prev.owner === e.owner && prev.targetId === e.target_id;
          this.activeTakeover = {
            id: same ? prev.id : undefined,
            state: e.state,
            owner: e.owner,
            targetId: e.target_id,
            reason: e.reason ?? null
          };
        }
        if (this.deps.client.getControlState) {
          try {
            const ticket = this.deps.issueSnapshotTicket?.();
            this.applyControlDto(await this.deps.client.getControlState(), ticket);
          } catch {
            // the next refresh / heartbeat re-syncs
          }
        }
        return;
      case "run_started":
        this.active.add(e.run_id);
        this.noteStarted(e.run_id, env.ts_ms);
        await this.ensureDriving(e.bot_id, e.run_id);
        return;
      case "run_finished":
        this.active.delete(e.run_id);
        if (this.active.size === 0 && this.mode !== "idle") {
          // `active` is empty, so the idle state this causes cancels nothing.
          await this.stopNativeRun();
        }
        return;
      case "control_requested":
        if (
          this.handled.has(e.request_id) ||
          this.deps.now() - env.ts_ms > STALE_REQUEST_MS
        ) {
          return;
        }
        this.handled.add(e.request_id);
        this.active.add(e.run_id);
        // Creation time unknown (the restore sweep corrects it): treated as
        // pre-existing, so a Stop never misses a run that was already going.
        this.noteStarted(e.run_id, -Infinity);
        await this.answer(e);
        return;
      default:
        return;
    }
  }

  private async answer(
    e: Extract<DaemonEvent, { type: "control_requested" }>
  ): Promise<void> {
    // I2: no local overlay and no local input for a run whose scope is unknown.
    const scope = await this.awaitScope({ runId: e.run_id, botId: e.bot_id });
    if (scope === null) {
      try {
        await this.deps.client.decideControl(
          e.request_id,
          "refuse",
          "The app has not identified which screen this action targets yet, so it was not performed. Retry the action in a moment."
        );
      } catch {
        // 404: the daemon already timed out
      }
      return;
    }
    if (scope !== "local_desktop") {
      // Browser and remote targets MUST NEVER highlight or click the local PC desktop.
      const takeoverBlocks =
        this.activeTakeover !== null &&
        (this.activeTakeover.state === "active" ||
          this.activeTakeover.state === "acquiring" ||
          this.activeTakeover.state === "paused");
      const proceed =
        this.mode !== "human_driving" &&
        this.mode !== "paused" &&
        !takeoverBlocks;
      if (proceed) {
        this.deps.panelOverlay?.showAction({
          runId: e.run_id,
          botId: e.bot_id,
          requestId: e.request_id,
          rect: e.rect,
          label: e.label,
          action: e.action,
          timestampMs: this.deps.now()
        });
      }
      const blockedMode: ControlModeName =
        this.activeTakeover?.state === "paused" || this.mode === "paused"
          ? "paused"
          : "human_driving";
      try {
        await this.deps.client.decideControl(
          e.request_id,
          proceed ? "proceed" : "refuse",
          proceed ? undefined : refusalReason(blockedMode)
        );
      } catch {
        // 404: the daemon already timed out; nothing to do
      }
      return;
    }

    await this.ensureDriving(e.bot_id, e.run_id);
    let proceed = false;
    try {
      proceed = await this.deps.native.beginAction(e.rect, e.label, e.action);
    } catch {
      proceed = false;
    }
    if (this.disposed) return;
    if (proceed && e.rect && e.action === "click") {
      const c = centre(e.rect);
      await this.deps.native.click(c.x, c.y).catch(() => undefined);
    }
    if (this.disposed) return;
    try {
      await this.deps.client.decideControl(
        e.request_id,
        proceed ? "proceed" : "refuse",
        proceed ? undefined : refusalReason(this.mode)
      );
    } catch {
      // 404: the daemon already timed out; nothing to do
    }
  }
}

/**
 * Owner of the single `ControlBridge` for the live-view controller. Its
 * identity never changes, so control-state refreshes only *sync* the existing
 * bridge (its live-run set, `startedAt` and mode must survive them).
 */
export interface BridgeRegistry {
  register(bridge: ControlBridge | null): void;
  current(): ControlBridge | null;
  syncControlState(dto: ControlStateDto): void;
  setOwnedTakeoverId(id: string | null): void;
}

export function createBridgeRegistry(
  getControlState: () => ControlStateDto | null
): BridgeRegistry {
  let bridge: ControlBridge | null = null;
  let ownedId: string | null = null;
  return {
    register(next) {
      bridge = next;
      if (next) {
        next.setOwnedTakeoverId(ownedId);
        const dto = getControlState();
        if (dto) {
          void next.syncControlState(dto);
        }
      }
    },
    current: () => bridge,
    syncControlState(dto) {
      void bridge?.syncControlState(dto);
    },
    setOwnedTakeoverId(id) {
      ownedId = id;
      bridge?.setOwnedTakeoverId(id);
    }
  };
}
