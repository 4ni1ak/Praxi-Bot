import assert from "node:assert/strict";
import { test } from "node:test";
import type { ControlModeName, ControlStateDto, DaemonEvent } from "../api/types.ts";
import { createInitialLiveViewState, shouldOverrideOnStop } from "../lib/liveViewState.ts";
import { ControlBridge, type ScopeAnswer } from "./controlBridge.ts";
import { FakeClock, FakeDaemon, FakeNative, flush, makeLedger } from "./ledgerFakes.ts";
import type { StopOrigin } from "./runLedger.ts";
import type { NativeControlSnapshot } from "./stopHost.ts";
import { describeStop } from "./stopStatus.ts";
import { LIST_STALE_MS, StopRuntime } from "./stopRuntime.ts";
import { TakeoverStop } from "./takeoverStop.ts";

/**
 * The installed wiring (review r-94 #13): StopRuntime (what ControlBridgeHost forwards
 * to) + the real ControlBridge + the real RunLedger + the real TakeoverStop, with fakes
 * only at the native (Tauri commands / events) and daemon (HTTP) boundary.
 */
function host(opts: { scope?: () => ScopeAnswer; runs?: [string, "running" | "paused"][] } = {}) {
  const clock = new FakeClock();
  const native = new FakeNative();
  const { ledger } = makeLedger({ clock, native });
  const daemon = new FakeDaemon();
  for (const [id, state] of opts.runs ?? []) {
    daemon.start(id, state);
  }
  let readFails = false;
  const runtime = new StopRuntime({
    ledger,
    readNative: async () =>
      (readFails ? { mode: "idle" } : native.snapshot()) as NativeControlSnapshot,
    now: () => clock.now()
  });
  // LiveViewContext's takeover half, with the daemon's control endpoint faked.
  let control: ControlStateDto | null = null;
  const overrides: string[] = [];
  const takeover = new TakeoverStop<string>({
    client: () => "daemon",
    controlled: () =>
      control ? shouldOverrideOnStop(createInitialLiveViewState(null), control) : null,
    post: async (c) => {
      overrides.push(c);
      return control as ControlStateDto;
    },
    apply: () => {},
    schedule: (fn, ms) => clock.set(fn, ms)
  });
  runtime.setOverride((o) => takeover.stop(!o.onlyIfControlled));

  const listCalls: { resolve: () => void }[] = [];
  let holdLists = false;
  const client = {
    stopRuns: daemon.transport().stopRuns,
    listRuns: () =>
      new Promise<ReturnType<FakeDaemon["list"]>>((resolve) => {
        const snapshot = daemon.list(); // captured when the daemon handles the request
        const done = () => resolve(snapshot);
        listCalls.push({ resolve: done });
        if (!holdLists) {
          done();
        }
      })
  };
  const calls = { start: 0, decide: [] as string[], clicks: 0 };
  let clickGate: Promise<void> | null = null;
  const scope = opts.scope ?? (() => "local_desktop" as ScopeAnswer);
  const makeBridge = () =>
    new ControlBridge({
      client: {
        putControlState: async () => undefined,
        decideControl: async (_id, decision) => void calls.decide.push(decision)
      },
      native: {
        beginAction: async () => native.mode === "bot_driving",
        startRun: async () => {
          calls.start += 1;
          const snap = native.start();
          runtime.onNativeSnapshot(snap as NativeControlSnapshot);
          return snap.runGeneration;
        },
        finishRun: async (g) => {
          runtime.onNativeSnapshot(native.finish(g) as NativeControlSnapshot);
        },
        click: async () => {
          calls.clicks += 1;
          await clickGate;
        },
        confirmDriving: () => runtime.confirmDriving()
      },
      ledger,
      resolveTargetScope: () => scope(),
      botMeta: (id) => ({ botId: id, botName: id }),
      startingStep: "x",
      now: () => 1_000_000,
      sleep: async () => {
        clock.advance(250);
      }
    });
  /** The host's native subscription: deliver queued events. */
  const pumpNative = () => {
    for (const snap of native.events.splice(0)) {
      runtime.onNativeSnapshot(snap as NativeControlSnapshot);
    }
  };
  let gseq = 0;
  const event = (e: DaemonEvent) => {
    gseq += 1;
    runtime.onDaemonEvent({ v: 1, seq: gseq, gseq, ts_ms: 1_000_000, event: e });
  };
  return {
    clock,
    native,
    ledger,
    daemon,
    runtime,
    takeover,
    overrides,
    calls,
    listCalls,
    makeBridge,
    pumpNative,
    event,
    setControl: (d: ControlStateDto | null) => {
      control = d;
      takeover.pump();
    },
    holdLists: (v: boolean) => {
      holdLists = v;
    },
    failReads: (v: boolean) => {
      readFails = v;
    },
    holdClick: () => {
      let release: () => void = () => {};
      clickGate = new Promise((r) => {
        release = r;
      });
      return () => release();
    },
    client,
    status: () =>
      describeStop({
        records: ledger.stopRecords(),
        runs: ledger.runEntries(),
        now: clock.now(),
        connected: ledger.client() !== null
      })
  };
}

function activeTakeover(): ControlStateDto {
  return {
    app_connected: true,
    expires_in_ms: 15_000,
    latched: "human_driving",
    mode: "human_driving",
    release: null,
    takeover: {
      id: "to-1",
      control_epoch: 1,
      input_expires_in_ms: 10_000,
      owner: "local-ui:1",
      session_id: "s",
      state: "active",
      target_id: "t"
    }
  };
}

type Press = (h: ReturnType<typeof host>) => void;
const ENTRY_POINTS: [string, Press, StopOrigin, boolean][] = [
  ["App hotkey (requestStop)", (h) => void h.runtime.requestStop("hotkey"), "hotkey", true],
  ["run banner (requestStop)", (h) => void h.runtime.requestStop("banner"), "banner", true],
  ["live viewer (requestStop)", (h) => void h.runtime.requestStop("viewer"), "viewer", true],
  [
    "widget / tray / global hotkey (native stop_all, event through the host subscription)",
    (h) => {
      h.native.userStop(null);
      h.pumpNative();
    },
    "native",
    false
  ]
];

test("every Stop entry point through the installed host: one decision, native halted, daemon asked, takeover input halted", async () => {
  for (const mode of ["bot_driving", "human_driving", "paused", "idle"] as ControlModeName[]) {
    for (const [name, press, origin, tokened] of ENTRY_POINTS) {
      const h = host({ runs: [["r1", "paused"]] });
      h.native.mode = mode;
      await h.runtime.readNative();
      const detach = h.runtime.attach(h.client, h.makeBridge());
      await flush();
      h.setControl(activeTakeover());
      press(h);
      h.pumpNative();
      await flush();
      const label = `${name} @ ${mode}`;
      const recs = h.ledger.stopRecords();
      assert.equal(recs.length, 1, label);
      assert.equal(recs[0]?.origin, origin, label);
      assert.equal(h.native.mode, "idle", `${label}: native halted`);
      assert.equal(h.native.tokens.length, 1, `${label}: one native stop`);
      assert.equal(h.native.tokens[0] === recs[0]?.id, tokened, `${label}: token`);
      assert.equal(h.daemon.requests.length, 1, `${label}: one daemon request`);
      assert.equal(h.ledger.mayAct("r1"), false, `${label}: run held`);
      assert.equal(h.takeover.inputAllowed(), false, `${label}: takeover input halted`);
      assert.equal(h.overrides.length, 1, `${label}: takeover override sent once`);
      h.daemon.deliver(0);
      await flush();
      assert.equal(h.status()?.kind, "stopped", label);
      assert.deepEqual(h.native.acks, [1], `${label}: acked after terminal`);
      detach();
    }
  }
});

test("chat Stop through the host cancels only its run, with no native or takeover effect", async () => {
  const h = host({ runs: [["r1", "running"], ["r2", "running"]] });
  await h.runtime.readNative();
  h.runtime.attach(h.client, h.makeBridge());
  await flush();
  h.runtime.cancelOne("r1");
  assert.deepEqual(h.daemon.requests[0]?.body.run_ids, ["r1"]);
  assert.equal(h.native.tokens.length, 0);
  assert.equal(h.takeover.inputAllowed(), true);
  assert.equal(h.ledger.mayAct("r2"), true);
});

test("a Stop during a local click through the host refuses the pending proceed (r-94 #1)", async () => {
  const h = host();
  await h.runtime.readNative();
  h.runtime.attach(h.client, h.makeBridge());
  await flush();
  const release = h.holdClick();
  h.event({ type: "control_requested", request_id: "q", run_id: "r", bot_id: "b", rect: { x: 0, y: 0, width: 2, height: 2 }, label: "Go", action: "click" });
  await flush();
  assert.equal(h.calls.clicks, 1);
  h.runtime.requestStop("banner");
  release();
  await flush();
  assert.deepEqual(h.calls.decide, ["refuse"]);
});

test("the global hotkey firing during a local action is caught by the host's last-moment read (r-94 #1)", async () => {
  const h = host();
  await h.runtime.readNative();
  h.runtime.attach(h.client, h.makeBridge());
  await flush();
  const release = h.holdClick();
  h.event({ type: "control_requested", request_id: "q", run_id: "r", bot_id: "b", rect: { x: 0, y: 0, width: 2, height: 2 }, label: "Go", action: "click" });
  await flush();
  h.native.userStop(null); // native stopped; its event is not delivered yet
  release();
  await flush();
  assert.deepEqual(h.calls.decide, ["refuse"]);
  assert.equal(h.ledger.stopRecords().length, 1, "the read revealed the Stop");
});

test("an older native revision never reaches the bridge or the host cache (r-94 #8)", async () => {
  const h = host();
  h.native.mode = "bot_driving";
  const older = { ...h.native.touch(), mode: "bot_driving" } as NativeControlSnapshot; // rev 1
  h.native.mode = "idle";
  const newer = { ...h.native.touch(), mode: "idle" } as NativeControlSnapshot; // rev 2
  h.runtime.onNativeSnapshot(newer);
  h.runtime.onNativeSnapshot(older); // delivered late
  assert.equal(h.runtime.nativeSnapshot()?.mode, "idle");
  const bridge = h.makeBridge();
  h.runtime.attach(h.client, bridge); // a replacement bridge starts from the cache
  await flush();
  assert.equal(bridge.mode, "idle");
});

test("a failed native read is never applied as Idle: the human keeps the wheel (r-94 #8)", async () => {
  const h = host({ runs: [["r1", "running"]] });
  const bridge = h.makeBridge();
  h.runtime.attach(h.client, bridge);
  await flush();
  h.native.takeWheel();
  h.pumpNative();
  await flush();
  assert.equal(bridge.mode, "human_driving");
  const startsBefore = h.calls.start;
  h.failReads(true);
  await h.runtime.readNative(); // getControlState() failed -> INITIAL fallback
  await flush();
  assert.equal(bridge.mode, "human_driving");
  h.runtime.scopeChanged(); // restore must not take the wheel back
  await flush();
  assert.equal(h.calls.start, startsBefore);
});

test("the host keeps one run list in flight; a stale one does not block forever (r-94 #3)", async () => {
  const h = host();
  h.holdLists(true);
  h.runtime.attach(h.client, null);
  h.runtime.tick();
  h.runtime.tick();
  assert.equal(h.listCalls.length, 1, "one list at a time");
  h.clock.advance(LIST_STALE_MS + 1);
  h.runtime.tick();
  assert.equal(h.listCalls.length, 2, "a hung list is superseded (tickets keep it ordered)");
});

test("restore drives a local run once the scope becomes known, without another daemon event (r-94 #10)", async () => {
  let scope: ScopeAnswer = "unknown";
  const h = host({ scope: () => scope, runs: [["r1", "running"]] });
  await h.runtime.readNative();
  h.runtime.attach(h.client, h.makeBridge());
  await flush();
  assert.equal(h.calls.start, 0, "list applied before targets/settings: waits");
  scope = "local_desktop";
  h.runtime.scopeChanged(); // what the host calls when settings / this client's targets load
  await flush();
  assert.equal(h.calls.start, 1);
});

test("Stop with no daemon client: input halts at once and the status says what the late Stop will cancel (r-94 #2, #12)", async () => {
  const h = host();
  await h.runtime.readNative();
  h.runtime.requestStop("hotkey");
  assert.equal(h.native.mode, "idle");
  assert.equal(h.takeover.inputAllowed(), false);
  const s = h.status();
  assert.equal(s?.kind, "unreachable");
  assert.equal(s?.pendingSweep, true, "first delivery will stop every run open on arrival");
  // the override is owed until a client (and its snapshot) exists
  assert.equal(h.takeover.owes(), true);
});
