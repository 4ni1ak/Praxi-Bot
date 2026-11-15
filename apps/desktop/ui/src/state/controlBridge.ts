import type {
  ControlModeName,
  ControlStateDto,
  ControlVerdict,
  DaemonEvent,
  GlobalEnvelope,
  ReleaseRequest,
  TakeoverInfo,
  TargetInfo,
  ViewReason
} from "../api/types.ts";

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
 *   - target scope not known yet -> wait (bounded), then refuse "retry in a moment";
 * - run lifecycle: a local-desktop run starts -> `bot_driving`; the last one ends ->
 *   `finish_bot_run(generation)` (a normal finish, never a Stop);
 * - legacy local control changes report via `PUT /v1/control/state`; plain
 *   heartbeats and reopens NEVER send `release: true`;
 * - while a daemon-owned takeover holds the latch, only `POST /v1/control/release`
 *   ("Bitirdim"), `POST /v1/control/override` (Stop) or `POST /v1/control/recover`
 *   touch the latch — heartbeats/reopen never clear it.
 *
 * The bridge never decides or performs a Stop: `RunLedger` (runLedger.ts) owns run
 * state and Stop records. Before EVERY effect (native call, daemon write) the bridge
 * re-checks that its authority is live and that the ledger lets the run act, so a
 * run a Stop holds, or a bridge of a replaced daemon client, never acts again.
 */

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type TargetScope = "local_desktop" | "browser" | "remote";
/** `unknown` until the CURRENT client's targets and the settings have loaded: never guessed. */
export type ScopeAnswer = TargetScope | "unknown";

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

/** What the bridge needs from the run ledger (`RunLedger` implements it). */
export interface BridgeLedger {
  mayAct(runId: string): boolean;
  /** Raised synchronously by every Stop decision (see `RunLedger.stopEpoch`). */
  stopEpoch(): number;
  observeRun(runId: string, botId: string | null, processing?: boolean): void;
  observeRunFinished(runId: string): void;
  claimRequest(requestId: string): boolean;
  runEntries(): readonly {
    id: string;
    botId: string | null;
    phase: string;
    processing: boolean;
  }[];
}

export interface BridgeDeps {
  client: {
    putControlState(mode: ControlModeName, release?: boolean): Promise<unknown>;
    decideControl(
      requestId: string,
      decision: ControlVerdict,
      reason?: string
    ): Promise<unknown>;
    getControlState?(): Promise<ControlStateDto>;
    releaseControl?(req?: ReleaseRequest): Promise<ControlStateDto>;
    overrideControl?(): Promise<ControlStateDto>;
    recoverControl?(): Promise<ControlStateDto>;
  };
  native: {
    beginAction(rect: Rect | null, label: string, action: string): Promise<boolean>;
    /** Resolves with the native run generation it started, when known. */
    startRun(bot: BotMeta, step: string): Promise<number | null | void>;
    /** Normal end of the native run started at `expectedGeneration` (never a Stop). */
    finishRun(expectedGeneration: number): Promise<void>;
    click(x: number, y: number): Promise<void>;
    /**
     * Fresh native read right before a local `proceed`: `true` only if native is still
     * bot driving and no Stop was pressed (the read is also routed to the ledger, so a
     * Stop it reveals raises the stop epoch). A failed read is `false`.
     */
    confirmDriving?(): Promise<boolean>;
  };
  ledger: BridgeLedger;
  /** Resolve whether a run / bot currently targets the local PC or a browser/remote screen. */
  resolveTargetScope?: (info: { runId?: string; botId?: string }) => ScopeAnswer;
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
  /** Wait helper (tests inject a fast one). */
  sleep?(ms: number): Promise<void>;
  /** Bound for daemon calls on the bridge queue (default `BRIDGE_CALL_BOUND_MS`). */
  callBoundMs?: number;
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
/** How long an action waits for the target scope before it is refused ("retry in a moment"). */
export const SCOPE_WAIT_MS = 3000;
const SCOPE_POLL_MS = 250;
/** No daemon call may hold the bridge queue longer than this. */
export const BRIDGE_CALL_BOUND_MS = 5000;

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

/** The app does not know yet which screen this run uses; the bot may try again shortly. */
export const SCOPE_UNKNOWN_REASON =
  "The app is still loading which screen this run uses, so this action was not performed. Retry it in a moment.";

function centre(r: Rect): { x: number; y: number } {
  return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
}

export class ControlBridge {
  mode: ControlModeName = "idle";
  /** Runs for which this bridge put native into bot_driving. */
  private readonly driving = new Set<string>();
  private readonly startedAt: number;
  private readonly deps: BridgeDeps;
  private chain: Promise<void> = Promise.resolve();
  private activeTakeover: TrackedTakeover | null = null;
  private releasingTakeover = false;
  /** Id of the takeover THIS app owns (set by the live-view controller). */
  private ownedTakeoverId: string | null = null;
  /** Native run generation of the run this bridge started (for `finish_bot_run`). */
  private generation: number | null = null;
  private authority = true;

  constructor(deps: BridgeDeps) {
    this.deps = deps;
    this.startedAt = deps.now();
  }

  /** False after `revoke()`: no further native call, daemon write or state write. */
  get live(): boolean {
    return this.authority;
  }

  /** Synchronously withdraw this bridge's authority (client replaced / unmounted). */
  revoke(): void {
    this.authority = false;
  }

  /** Runs the bridge is currently driving native for (tests / diagnostics). */
  drivingRuns(): string[] {
    return [...this.driving];
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
    return this.enqueue(async () => {
      const ticket = this.deps.issueSnapshotTicket?.();
      const dto = await this.bounded(get.call(this.deps.client));
      this.applyControlDto(dto, ticket);
    });
  }

  /** Sync authoritative daemon control snapshot into the bridge. */
  syncControlState(dto: ControlStateDto): Promise<void> {
    return this.enqueue(() => {
      this.applyControlDto(dto);
    });
  }

  /** Feed one daemon event; work is serialised in arrival order. */
  handleEvent(env: GlobalEnvelope): Promise<void> {
    return this.enqueue(() => this.process(env));
  }

  /**
   * The native state machine changed (Tauri `control:state_changed`). The host
   * only forwards snapshots the ledger found newer (`stateRev`).
   */
  onNativeState(mode: ControlModeName, generation?: number): Promise<void> {
    return this.enqueue(() => this.nativeState(mode, generation));
  }

  /**
   * Drive native for restored local runs (after the ledger applied a run list).
   * Restore never cancels anything; only Stop records do.
   */
  restoreDriving(): Promise<void> {
    return this.enqueue(async () => {
      const epoch = this.deps.ledger.stopEpoch();
      for (const run of this.deps.ledger.runEntries()) {
        if (run.phase === "tracked" && run.processing && run.botId) {
          await this.ensureDriving(run.botId, run.id, epoch);
        }
      }
    });
  }

  /**
   * Explicitly hand back an owned takeover ("Bitirdim") without cancelling live
   * runs. A failed release rejects (the caller decides what the daemon state
   * means); the internal serialisation chain stays usable.
   */
  releaseOwnedTakeover(req: ReleaseRequest): Promise<ControlStateDto | null> {
    const run = this.chain.then(async () => {
      if (!this.deps.client.releaseControl || !this.live) {
        return null;
      }
      this.releasingTakeover = true;
      try {
        const result = await this.bounded(this.deps.client.releaseControl(req));
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

  /** Send the current mode (call every `HEARTBEAT_MS` and on connect). */
  async heartbeat(release = false): Promise<void> {
    if (!this.live) {
      return;
    }
    // Heartbeats and reopens never release an owned takeover latch.
    const effectiveRelease = release && !this.hasOwnedTakeover();
    try {
      await this.bounded(
        this.deps.client.putControlState(this.mode, effectiveRelease || undefined)
      );
    } catch {
      // daemon unreachable or 409 because owned takeover holds the latch
    }
  }

  private enqueue(work: () => void | Promise<void>): Promise<void> {
    this.chain = this.chain
      .then(() => (this.live ? work() : undefined))
      .catch(() => undefined);
    return this.chain;
  }

  /** No daemon call holds the queue forever: a hung request is abandoned, not awaited. */
  private bounded<T>(p: Promise<T>): Promise<T> {
    const ms = this.deps.callBoundMs ?? BRIDGE_CALL_BOUND_MS;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const limit = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error("bridge call bound exceeded")), ms);
    });
    return Promise.race([p, limit]).finally(() => clearTimeout(timer));
  }

  private sleep(ms: number): Promise<void> {
    return this.deps.sleep
      ? this.deps.sleep(ms)
      : new Promise((resolve) => setTimeout(resolve, ms));
  }

  /** May this bridge still affect `runId`? Checked before every effect. */
  private mayAct(runId: string): boolean {
    return this.live && this.deps.ledger.mayAct(runId);
  }

  /**
   * The synchronous Stop gate: the session is live, the run may act AND no Stop was
   * decided since the work began (`epoch`). Evaluated immediately before each effect.
   */
  private allowed(runId: string, epoch: number): boolean {
    return this.mayAct(runId) && this.deps.ledger.stopEpoch() === epoch;
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
    if (!this.live) {
      return; // a revoked bridge's late replies write nothing
    }
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

  private scopeFor(info: { runId?: string; botId?: string }): ScopeAnswer {
    return this.deps.resolveTargetScope?.(info) ?? "local_desktop";
  }

  /** Wait (bounded) until the scope is known; `unknown` after `SCOPE_WAIT_MS`. */
  private async waitForScope(info: { runId?: string; botId?: string }): Promise<ScopeAnswer> {
    let scope = this.scopeFor(info);
    let waited = 0;
    while (scope === "unknown" && waited < SCOPE_WAIT_MS && this.live) {
      await this.sleep(SCOPE_POLL_MS);
      waited += SCOPE_POLL_MS;
      scope = this.scopeFor(info);
    }
    return scope;
  }

  private async nativeState(mode: ControlModeName, generation?: number): Promise<void> {
    const previous = this.mode;
    this.mode = mode;
    if (generation !== undefined) {
      this.generation = generation;
    }
    if (mode === "idle") {
      // Native drives nothing any more (a Stop or a finish): nothing left to finish.
      this.driving.clear();
    }

    // While an owned takeover controls the daemon latch, a local Pause overrides
    // instead of fighting the latch, and leaving human_driving never sends legacy release:true.
    if (mode === "paused" && this.hasOwnedTakeover() && this.deps.client.overrideControl) {
      try {
        const dto = await this.bounded(this.deps.client.overrideControl());
        this.applyControlDto(dto);
      } catch {
        // next beat / state check retries
      }
      return;
    }
    const resumed =
      !this.hasOwnedTakeover() &&
      (previous === "human_driving" || previous === "paused") &&
      (mode === "idle" || mode === "bot_driving");
    await this.heartbeat(resumed);
  }

  private fresh(env: GlobalEnvelope): boolean {
    if (env.event.type === "control_state_changed") {
      return true;
    }
    return env.ts_ms >= this.startedAt - REPLAY_SLACK_MS;
  }

  /** Put native into bot_driving for a LOCAL run that may act. Unknown scope never does. */
  private async ensureDriving(botId: string, runId: string, epoch: number): Promise<void> {
    if (!this.allowed(runId, epoch) || this.scopeFor({ botId, runId }) !== "local_desktop") {
      return;
    }
    if (this.mode === "idle") {
      const generation = await this.deps.native.startRun(
        this.deps.botMeta(botId),
        this.deps.startingStep
      );
      if (typeof generation === "number") {
        this.generation = generation;
      }
      // The state machine reports bot_driving through `onNativeState`; do not
      // wait for it to decide this request.
      this.mode = "bot_driving";
    }
    if (this.mode === "bot_driving") {
      this.driving.add(runId);
    }
  }

  /** The last run this bridge drove ended normally: finish (never Stop) native. */
  private async finishIfLast(runId: string): Promise<void> {
    const had = this.driving.delete(runId);
    if (!had || this.driving.size > 0 || this.mode !== "bot_driving" || !this.live) {
      return;
    }
    const generation = this.generation;
    this.mode = "idle";
    if (generation !== null) {
      // A no-op natively when a newer run took over the generation meanwhile.
      await this.deps.native.finishRun(generation).catch(() => undefined);
    }
  }

  private async process(env: GlobalEnvelope): Promise<void> {
    const e: DaemonEvent = env.event;
    if (e.type === "run_finished") {
      // Terminal is always true, even from replayed history.
      this.deps.ledger.observeRunFinished(e.run_id);
      await this.finishIfLast(e.run_id);
      return;
    }
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
            const dto = await this.bounded(this.deps.client.getControlState());
            this.applyControlDto(dto, ticket);
          } catch {
            // the next refresh / heartbeat re-syncs
          }
        }
        return;
      case "run_started":
        this.deps.ledger.observeRun(e.run_id, e.bot_id);
        await this.ensureDriving(e.bot_id, e.run_id, this.deps.ledger.stopEpoch());
        return;
      case "control_requested":
        if (this.deps.now() - env.ts_ms > STALE_REQUEST_MS) {
          return;
        }
        if (!this.deps.ledger.claimRequest(e.request_id)) {
          return;
        }
        this.deps.ledger.observeRun(e.run_id, e.bot_id);
        await this.answer(e);
        return;
      default:
        return;
    }
  }

  /**
   * Post the verdict. A `proceed` is re-checked against the Stop gate synchronously,
   * right before the request, so nothing a Stop decided meanwhile can still proceed.
   */
  private async decide(
    requestId: string,
    proceed: boolean,
    reason: string | undefined,
    gate: () => boolean
  ): Promise<void> {
    if (!this.live) {
      return; // a revoked bridge writes nothing; the daemon times the request out
    }
    let verdict = proceed;
    let why = reason;
    if (verdict && !gate()) {
      verdict = false;
      why = refusalReason("idle");
    }
    try {
      await this.bounded(
        this.deps.client.decideControl(requestId, verdict ? "proceed" : "refuse", verdict ? undefined : why)
      );
    } catch {
      // 404: the daemon already timed out; nothing to do
    }
  }

  private async answer(
    e: Extract<DaemonEvent, { type: "control_requested" }>
  ): Promise<void> {
    const runId = e.run_id;
    const epoch = this.deps.ledger.stopEpoch();
    const gate = () => this.allowed(runId, epoch);
    if (!gate()) {
      // Stop-held, quarantined behind an unanswered Stop, or revoked: never proceeds.
      await this.decide(e.request_id, false, refusalReason("idle"), gate);
      return;
    }
    const scope = await this.waitForScope({ runId, botId: e.bot_id });
    if (scope === "unknown") {
      await this.decide(e.request_id, false, SCOPE_UNKNOWN_REASON, gate);
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
        !takeoverBlocks &&
        gate();
      if (proceed) {
        this.deps.panelOverlay?.showAction({
          runId,
          botId: e.bot_id,
          requestId: e.request_id,
          rect: e.rect,
          label: e.label,
          action: e.action,
          timestampMs: this.deps.now()
        });
      }
      const blockedMode: ControlModeName = !gate()
        ? "idle"
        : this.activeTakeover?.state === "paused" || this.mode === "paused"
          ? "paused"
          : "human_driving";
      await this.decide(e.request_id, proceed, refusalReason(blockedMode), gate);
      return;
    }

    await this.ensureDriving(e.bot_id, runId, epoch);
    let proceed = false;
    if (gate()) {
      try {
        proceed = await this.deps.native.beginAction(e.rect, e.label, e.action);
      } catch {
        proceed = false;
      }
    }
    if (proceed && e.rect && e.action === "click" && gate()) {
      const c = centre(e.rect);
      await this.deps.native.click(c.x, c.y).catch(() => undefined);
    }
    // The global hotkey may have stopped native while we waited: read it fresh.
    if (proceed && gate() && this.deps.native.confirmDriving) {
      proceed = await this.deps.native.confirmDriving().catch(() => false);
    }
    await this.decide(
      e.request_id,
      proceed,
      refusalReason(gate() ? this.mode : "idle"),
      gate
    );
  }
}

/**
 * Owner of the single `ControlBridge` for the live-view controller. Its
 * identity never changes, so control-state refreshes only *sync* the existing
 * bridge (its mode and takeover must survive them).
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
