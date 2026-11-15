import assert from "node:assert/strict";
import { test } from "node:test";
import type { ControlModeName, GlobalEnvelope, DaemonEvent } from "../api/types.ts";
import { BridgeSession } from "./bridgeSession.ts";
import { ControlBridge, StopLedger } from "./controlBridge.ts";

/**
 * Model test: drives the bridge + start-up session through seeded, random
 * interleavings of native events, snapshot/list/cancel resolution, client swaps,
 * settle events and new runs, and checks after every sequence:
 *  I1 a cancel is only ever sent for a run that existed at some user Stop;
 *  I2 startRun / beginAction never happen while the scope is unknown for the
 *     CURRENT client;
 *  I3 after a Stop each targeted run is cancelled, finished or tracked by the
 *     current bridge;
 *  I4 every user Stop reaches every run that existed at that moment (a dropped
 *     Stop shows up as a run that was never sent a cancel).
 */

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface SimRun {
  id: string;
  created: number;
  kind: "browser" | "local";
  state: "running" | "cancelled" | "finished";
}

class Sim {
  now = 1_000_000;
  runs = new Map<string, SimRun>();
  native = { mode: "idle" as ControlModeName, gen: 1 };
  stopTimes: number[] = [];
  cancelSent = new Set<string>();
  violations: string[] = [];
  log: string[] = [];
  ledger = new StopLedger();
  sessions: { session: BridgeSession; bridge: ControlBridge; index: number }[] = [];
  settled = new Map<number, { targets: boolean }>();
  settingsLoaded = false;
  pendingSnapshots: (() => void)[] = [];
  pendingLists: (() => void)[] = [];
  pendingCancels: { id: string; ok: (ok: boolean) => void }[] = [];
  pendingWaits: (() => void)[] = [];
  inflight: Promise<unknown>[] = [];
  nextRun = 0;
  private gseq = 0;

  get current() {
    return this.sessions[this.sessions.length - 1];
  }

  scopeKnown(index: number): boolean {
    return (this.settled.get(index)?.targets ?? false) && this.settingsLoaded;
  }

  emit(): void {
    if (process.env.MODEL_TRACE) console.log(`  emit ${this.native.mode}#${this.native.gen} t=${this.now} -> session${this.current?.index}`);
    this.current?.session.onNativeEvent(this.native.mode, this.native.gen);
  }

  wait = (p: Promise<unknown>): Promise<void> =>
    new Promise<void>((resolve) => {
      p.then(
        () => resolve(),
        () => resolve()
      );
      this.pendingWaits.push(resolve);
    });

  makeSession(): void {
    const index = this.sessions.length;
    // eslint-disable-next-line @typescript-eslint/no-this-alias -- closures below need the simulator
    const sim = this;
    const prev = this.current;
    const bridge = new ControlBridge({
      client: {
        listRuns: () =>
          new Promise((resolve) => {
            sim.pendingLists.push(() =>
              resolve(
                [...sim.runs.values()]
                  .filter((r) => r.state === "running")
                  .map((r) => ({
                    id: r.id,
                    bot_id: `bot-${r.kind}`,
                    state: "running",
                    created_at_ms: r.created
                  }))
              )
            );
          }),
        putControlState: async () => undefined,
        decideControl: async () => undefined,
        cancelRun: (id: string) => {
          const run = sim.runs.get(id);
          const valid = !!run && sim.stopTimes.some((t) => run.created <= t);
          if (!valid) sim.violations.push(`I1: cancel ${id}(created ${run?.created}) by bridge${index} at op#${sim.log.length}=${sim.log[sim.log.length - 1]} now=${sim.now} stops=[${sim.stopTimes}]`);
          sim.cancelSent.add(id);
          if (process.env.MODEL_TRACE) console.log(`  cancelRun ${id} by bridge${index}`);
          return new Promise((resolve, reject) => {
            sim.pendingCancels.push({
              id,
              ok: (ok) => {
                if (ok) {
                  if (run && run.state === "running") run.state = "cancelled";
                  resolve(undefined);
                } else {
                  reject(new Error("cancel failed"));
                }
              }
            });
          });
        }
      },
      native: {
        beginAction: async () => {
          if (!sim.scopeKnown(index)) sim.violations.push("I2: beginAction with unknown scope");
          return sim.native.mode === "bot_driving";
        },
        startRun: async () => {
          if (!sim.scopeKnown(index)) sim.violations.push("I2: startRun with unknown scope");
          if (sim.native.mode === "idle") {
            sim.native = { mode: "bot_driving", gen: sim.native.gen + 1 };
            sim.emit();
          }
        },
        stopRun: async () => {
          sim.native = { mode: "idle", gen: sim.native.gen + 1 };
          sim.emit(); // the echo of a bridge-caused finish: NOT a user Stop
          return sim.native.gen;
        },
        click: async () => undefined
      },
      resolveTargetScope: ({ runId }) =>
        sim.runs.get(runId ?? "")?.kind === "browser" ? "browser" : "local_desktop",
      targetsLoaded: () => sim.scopeKnown(index),
      botMeta: (id) => ({ botId: id, botName: id }),
      startingStep: "x",
      now: () => sim.now,
      ledger: sim.ledger,
      scopeWaitMs: 0,
      wait: (p) => sim.wait(p)
    });
    const session = new BridgeSession(bridge, {
      getNativeSnapshot: () => {
        const snap = { mode: sim.native.mode, runGeneration: sim.native.gen };
        return new Promise((resolve) => sim.pendingSnapshots.push(() => resolve(snap)));
      },
      retiring: prev ? prev.session.retiredDone : Promise.resolve(),
      ledger: sim.ledger,
      wait: (p) => sim.wait(p),
      now: () => sim.now
    });
    this.sessions.push({ session, bridge, index });
    this.settled.set(index, { targets: false });
  }

  deliver(event: DaemonEvent): void {
    const env: GlobalEnvelope = { v: 1, seq: 1, ts_ms: this.now, gseq: ++this.gseq, event };
    void this.current?.bridge.handleEvent(env);
  }

  async flush(): Promise<void> {
    for (let i = 0; i < 4; i += 1) await new Promise((r) => setImmediate(r));
  }
}

const OPS = [
  "stop", "stop", "hotkey", "takeWheel", "giveBack", "newRun", "newRun", "finishRun",
  "tick", "tick", "resolveSnapshot", "resolveList", "cancelOk", "cancelFail", "bound",
  "settleTargets", "settleSettings", "swap", "controlRequest"
] as const;

async function step(sim: Sim, op: (typeof OPS)[number], rnd: () => number): Promise<void> {
  sim.now += 1;
  sim.log.push(op);
  if (process.env.MODEL_TRACE) console.log(`op#${sim.log.length} ${op}`);
  const pick = <T>(list: T[]): T | undefined =>
    list.length ? list.splice(Math.floor(rnd() * list.length), 1)[0] : undefined;
  switch (op) {
    case "stop":
      sim.native = { mode: "idle", gen: sim.native.gen + 1 };
      sim.stopTimes.push(sim.now);
      sim.emit();
      break;
    case "hotkey":
      if (sim.native.mode === "idle") sim.emit();
      break;
    case "takeWheel":
      if (sim.native.mode === "bot_driving") {
        sim.native = { mode: "human_driving", gen: sim.native.gen };
        sim.emit();
      }
      break;
    case "giveBack":
      if (sim.native.mode === "human_driving") {
        sim.native = { mode: "bot_driving", gen: sim.native.gen };
        sim.emit();
      }
      break;
    case "newRun": {
      const id = `run-${sim.nextRun++}`;
      const run: SimRun = { id, created: sim.now, kind: rnd() < 0.5 ? "browser" : "local", state: "running" };
      sim.runs.set(id, run);
      sim.deliver({ type: "run_started", run_id: id, chat_id: "c", bot_id: `bot-${run.kind}` });
      break;
    }
    case "finishRun": {
      const live = [...sim.runs.values()].filter((r) => r.state === "running");
      const r = live[Math.floor(rnd() * live.length)];
      if (r) {
        r.state = "finished";
        sim.deliver({ type: "run_finished", run_id: r.id, error: null });
      }
      break;
    }
    case "tick":
      sim.inflight.push(sim.current.session.tick());
      break;
    case "resolveSnapshot":
      pick(sim.pendingSnapshots)?.();
      break;
    case "resolveList":
      pick(sim.pendingLists)?.();
      break;
    case "cancelOk":
      pick(sim.pendingCancels)?.ok(true);
      break;
    case "cancelFail":
      pick(sim.pendingCancels)?.ok(false);
      break;
    case "bound":
      pick(sim.pendingWaits)?.();
      break;
    case "settleTargets":
      sim.settled.get(sim.current.index)!.targets = true;
      break;
    case "settleSettings":
      sim.settingsLoaded = true;
      break;
    case "swap":
      sim.current.session.dispose();
      sim.makeSession();
      break;
    case "controlRequest": {
      const live = [...sim.runs.values()].filter((r) => r.state === "running");
      const r = live[Math.floor(rnd() * live.length)];
      if (r) {
        sim.deliver({
          type: "control_requested",
          request_id: `req-${sim.now}`,
          run_id: r.id,
          bot_id: `bot-${r.kind}`,
          rect: { x: 1, y: 1, width: 2, height: 2 },
          label: "x",
          action: "click"
        });
      }
      break;
    }
  }
  await sim.flush();
}

async function quiesce(sim: Sim): Promise<void> {
  for (let round = 0; round < 12; round += 1) {
    sim.settled.get(sim.current.index)!.targets = true;
    sim.settingsLoaded = true;
    for (const f of sim.pendingSnapshots.splice(0)) f();
    for (const f of sim.pendingLists.splice(0)) f();
    for (const w of sim.pendingWaits.splice(0)) w();
    for (const c of sim.pendingCancels.splice(0)) c.ok(true);
    sim.inflight.push(sim.current.session.tick());
    await sim.flush();
  }
  await Promise.allSettled(sim.inflight);
}

async function runSequence(seed: number, length: number): Promise<Sim> {
  const rnd = mulberry32(seed);
  const sim = new Sim();
  // bootstrap: a first session that has seen the native state, like a running app
  sim.makeSession();
  sim.settled.get(0)!.targets = true;
  sim.settingsLoaded = rnd() < 0.5;
  sim.inflight.push(sim.current.session.tick());
  sim.pendingSnapshots.splice(0).forEach((f) => f());
  await sim.flush();
  for (let i = 0; i < length; i += 1) {
    await step(sim, OPS[Math.floor(rnd() * OPS.length)], rnd);
  }
  await quiesce(sim);
  return sim;
}

test("model: Stop / scope / cancel invariants hold across seeded interleavings", async () => {
  const failures: string[] = [];
  const only = process.env.MODEL_SEED ? Number(process.env.MODEL_SEED) : null;
  const seeds = Number(process.env.MODEL_SEEDS ?? 300);
  for (let seed = only ?? 1; seed <= (only ?? seeds); seed += 1) {
    const sim = await runSequence(seed, Number(process.env.MODEL_LEN ?? 40));
    const problems = [...sim.violations];
    // I3 + I4 for every run that existed at some user Stop
    for (const run of sim.runs.values()) {
      const targeted = sim.stopTimes.some((t) => run.created <= t);
      if (!targeted) continue;
      if (run.state === "finished") continue;
      if (!sim.cancelSent.has(run.id)) {
        problems.push(`I4: Stop never reached ${run.id}`);
      }
      const tracked = sim.current.bridge.liveRuns().includes(run.id);
      if (run.state !== "cancelled" && !tracked) {
        problems.push(`I3: ${run.id} neither cancelled nor tracked by the current bridge`);
      }
    }
    if (problems.length > 0) failures.push(`seed ${seed}: ${[...new Set(problems)].join("; ")} :: ${sim.log.join(",")}`);
  }
  assert.deepEqual(failures.slice(0, 5), [], `${failures.length} sequences violated an invariant`);
});
