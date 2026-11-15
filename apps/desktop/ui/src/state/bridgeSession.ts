import type { ControlModeName } from "../api/types.ts";
import {
  type ControlBridge,
  type NativeEvent,
  planNativeReplay,
  type StopLedger,
  waitBounded
} from "./controlBridge.ts";

/** Longest the next bridge waits for the retired bridge's queued cancels. */
export const RETIRING_WAIT_MS = 3000;

export interface SessionOptions {
  getNativeSnapshot(): Promise<{ mode: ControlModeName; runGeneration: number }>;
  /** Settles when the session this one replaces has issued everything it queued. */
  retiring: Promise<void>;
  ledger?: StopLedger;
  retiringWaitMs?: number;
  /** Injectable for tests; defaults to a timer. */
  wait?: (p: Promise<unknown>, ms: number) => Promise<void>;
  now?: () => number;
}

/**
 * One daemon client's bridge plus its start-up protocol. Invariants:
 *  I4 no native event is ever dropped: events before the initial snapshot are
 *     buffered and judged against it (never applied if older), and from the moment
 *     the snapshot is planned every event is queued on the bridge in order;
 *  a session torn down mid-init still flushes its buffer onto the bridge before
 *  retiring it, and a Stop it could not judge (no snapshot) is carried through
 *  the shared ledger rather than lost.
 */
export class BridgeSession {
  private buffered: NativeEvent[] = [];
  /** Highest generation received before this session existed (the walk's floor). */
  private readonly floorAtStart: number | null;
  private ready = false;
  private initializing = false;
  private disposed = false;
  private retired = false;
  private resolveRetired!: () => void;
  /** Resolves when this session's bridge has issued everything it queued. */
  readonly retiredDone: Promise<void> = new Promise<void>((r) => {
    this.resolveRetired = r;
  });

  readonly bridge: ControlBridge;
  private readonly opt: SessionOptions;

  constructor(bridge: ControlBridge, opt: SessionOptions) {
    this.bridge = bridge;
    this.opt = opt;
    this.floorAtStart = opt.ledger?.lastGeneration ?? null;
  }

  /** A native `control:state_changed`. */
  onNativeEvent(mode: ControlModeName, generation: number): void {
    if (this.disposed && this.retired) return;
    this.noteReceived(generation);
    if (!this.ready) {
      this.buffered.push({ mode, generation, at: this.now() });
      if (this.buffered.length > 200) this.buffered.shift();
      return;
    }
    void this.bridge.onNativeState(mode, generation, this.now());
  }

  private noteReceived(generation: number): void {
    const l = this.opt.ledger;
    if (l && (l.lastGeneration === null || generation > l.lastGeneration)) {
      l.lastGeneration = generation;
    }
  }

  private now(): number {
    return (this.opt.now ?? Date.now)();
  }

  private async applySnapshot(snap: {
    mode: ControlModeName;
    runGeneration: number;
  }): Promise<void> {
    this.noteReceived(snap.runGeneration); // the next session's floor includes it
    // The previous session may already have received an earlier generation: start
    // the walk there, so a Stop contained in this snapshot is still a rise.
    const plan = planNativeReplay(
      { mode: snap.mode, generation: snap.runGeneration },
      this.buffered,
      this.floorAtStart
    );
    this.buffered = [];
    const init = this.bridge.initializeNativeState(snap.mode, plan.baseline); // queued first
    this.ready = true; // from here every event queues behind it, in order
    for (const ev of plan.historicStops) {
      void this.bridge.applyHistoricStop(ev.generation, ev.at ?? this.now());
    }
    for (const ev of plan.replay) {
      void this.bridge.onNativeState(ev.mode, ev.generation, ev.at);
    }
    await init;
  }

  async tick(): Promise<void> {
    if (this.retired || this.initializing) return;
    if (!this.ready) {
      this.initializing = true;
      let snap: { mode: ControlModeName; runGeneration: number } | null = null;
      try {
        snap = await this.opt.getNativeSnapshot();
      } catch {
        // retry on the next tick without changing control
      }
      if (this.disposed) {
        // Torn down while the snapshot was in flight: flush before retiring.
        if (snap && !this.ready) {
          try {
            await this.applySnapshot(snap);
          } catch {
            // best effort
          }
        }
        this.initializing = false;
        this.retire();
        return;
      }
      try {
        if (snap) await this.applySnapshot(snap);
      } catch {
        // retry next tick
      } finally {
        this.initializing = false;
      }
      if (this.disposed) {
        this.retire();
        return;
      }
    }
    if (this.disposed || !this.ready) return;
    const wait = this.opt.wait ?? waitBounded;
    await wait(this.opt.retiring, this.opt.retiringWaitMs ?? RETIRING_WAIT_MS);
    if (this.disposed) return;
    void this.bridge.restoreRuns();
    void this.bridge.retryDeferredStarts();
    await this.bridge.heartbeat();
    void this.bridge.refreshControlState();
  }

  dispose(): void {
    this.disposed = true;
    if (!this.initializing) this.retire();
  }

  private retire(): void {
    if (this.retired) return;
    this.retired = true;
    const unjudged = !this.ready;
    this.bridge.dispose();
    void (async () => {
      if (unjudged && this.opt.ledger) {
        // An idle that is the echo of a stop a bridge itself caused is not the
        // user's: wait (bounded) until those generations are known.
        const wait = this.opt.wait ?? waitBounded;
        await wait(this.opt.ledger.whenAutoSettled(), this.opt.retiringWaitMs ?? RETIRING_WAIT_MS);
        this.carryUnjudgedStop();
      }
      await this.bridge.drain();
      this.resolveRetired();
    })();
  }

  /**
   * Buffered events that were never judged against a snapshot: a generation rise
   * inside the buffer is a stop_all (the only way to go idle), carried so the
   * next bridge's restore applies it to the runs that existed by then.
   */
  private carryUnjudgedStop(): void {
    const ledger = this.opt.ledger;
    if (!ledger) return;
    let high = this.floorAtStart ?? -Infinity;
    let cutoff: number | null = null;
    for (const ev of this.buffered) {
      const auto = ledger.autoGenerations.has(ev.generation);
      if (ev.mode === "idle" && ev.generation > high && high !== -Infinity && !auto) {
        cutoff = Math.max(cutoff ?? ev.at ?? 0, ev.at ?? this.now());
      }
      high = Math.max(high, ev.generation);
    }
    if (cutoff !== null) ledger.noteStop(cutoff, this.bridge);
  }
}
