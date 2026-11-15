import assert from "node:assert/strict";
import { test } from "node:test";
import type {
  ControlModeName,
  ControlStateDto,
  ControlVerdict,
  DaemonEvent,
  GlobalEnvelope,
  ReleaseRequest,
  Run
} from "../api/types.ts";
import {
  type BotMeta,
  type BridgeDeps,
  ControlBridge,
  createBridgeRegistry,
  type PanelActionHighlight,
  planNativeReplay,
  type Rect,
  refusalReason,
  resolveTargetScope,
  type TargetScope,
  waitBounded
} from "./controlBridge.ts";

const NOW = 1_000_000;

interface Calls {
  put: ControlModeName[];
  release: boolean[];
  releaseTakeover: ReleaseRequest[];
  override: number;
  recover: number;
  decide: { id: string; decision: ControlVerdict; reason?: string }[];
  cancel: string[];
  begin: { rect: Rect | null; label: string; action: string }[];
  start: { bot: BotMeta; step: string }[];
  stop: number;
  clicks: { x: number; y: number }[];
  panelOverlay: PanelActionHighlight[];
}

function setup(
  beginResult: boolean | (() => boolean) = true,
  scopeResolver?: (info: { runId?: string; botId?: string }) => TargetScope,
  extra: Partial<BridgeDeps> = {}
) {
  const calls: Calls = {
    put: [],
    release: [],
    releaseTakeover: [],
    override: 0,
    recover: 0,
    decide: [],
    cancel: [],
    begin: [],
    start: [],
    stop: 0,
    clicks: [],
    panelOverlay: []
  };
  let now = NOW;
  const hold: { current: Promise<void> } = { current: Promise.resolve() };
  const cancelFail = { current: false };
  let listRuns: () => Promise<Pick<Run, "id" | "bot_id" | "state">[]> = async () => [];
  const idleDto = (): ControlStateDto => ({
    app_connected: true,
    expires_in_ms: 15_000,
    latched: null,
    mode: "idle",
    release: null,
    takeover: null
  });
  const bridge = new ControlBridge({
    client: {
      listRuns: () => listRuns(),
      putControlState: async (m, release) => {
        calls.put.push(m);
        calls.release.push(release === true);
        await hold.current;
      },
      decideControl: async (id, decision, reason) =>
        void calls.decide.push({ id, decision, reason }),
      cancelRun: async (id) => {
        if (cancelFail.current) {
          throw new Error("cancel failed");
        }
        calls.cancel.push(id);
      },
      releaseControl: async (req) => {
        if (req) {
          calls.releaseTakeover.push(req);
        }
        return idleDto();
      },
      overrideControl: async () => {
        calls.override += 1;
        return {
          ...idleDto(),
          latched: "paused",
          mode: "paused"
        };
      },
      recoverControl: async () => {
        calls.recover += 1;
        return idleDto();
      }
    },
    native: {
      beginAction: async (rect, label, action) => {
        calls.begin.push({ rect, label, action });
        return typeof beginResult === "function" ? beginResult() : beginResult;
      },
      startRun: async (bot, step) => void calls.start.push({ bot, step }),
      stopRun: async () => ++calls.stop,
      click: async (x, y) => void calls.clicks.push({ x, y })
    },
    resolveTargetScope: scopeResolver,
    ...extra,
    panelOverlay: {
      showAction: (h) => calls.panelOverlay.push(h)
    },
    botMeta: (id) => ({ botId: id, botName: `Bot ${id}` }),
    startingStep: "Working…",
    now: () => now
  });
  let gseq = 0;
  const feed = (event: DaemonEvent, ts = now) => {
    gseq += 1;
    const env: GlobalEnvelope = { v: 1, seq: gseq, ts_ms: ts, gseq, event };
    return bridge.handleEvent(env);
  };
  return {
    bridge,
    calls,
    feed,
    snapshot: (read: typeof listRuns) => { listRuns = read; },
    hold,
    cancelFail,
    advance: (ms: number) => {
      now += ms;
    }
  };
}

const request = (id: string, rect: Rect | null = null, action = "click") =>
  ({
    type: "control_requested",
    request_id: id,
    run_id: "run-1",
    bot_id: "b1",
    rect,
    label: "Save",
    action
  }) satisfies DaemonEvent;

test("a started run makes the bot drive; the last finished run ends it", async () => {
  const t = setup();
  await t.feed({
    type: "run_started",
    run_id: "run-1",
    chat_id: "c",
    bot_id: "b1"
  });
  assert.equal(t.calls.start.length, 1);
  assert.equal(t.calls.start[0].bot.botName, "Bot b1");
  assert.equal(t.calls.start[0].step, "Working…");
  assert.equal(t.bridge.mode, "bot_driving");
  // a second run does not restart it
  await t.feed({
    type: "run_started",
    run_id: "run-2",
    chat_id: "c",
    bot_id: "b1"
  });
  assert.equal(t.calls.start.length, 1);
  await t.feed({ type: "run_finished", run_id: "run-1", error: null });
  assert.equal(t.calls.stop, 0, "run-2 is still live");
  await t.feed({ type: "run_finished", run_id: "run-2", error: null });
  assert.equal(t.calls.stop, 1);
  assert.equal(t.bridge.mode, "idle");
  assert.deepEqual(t.bridge.liveRuns(), []);
});

test("events from before the bridge existed (history replay) are ignored", async () => {
  const t = setup();
  await t.feed(
    { type: "run_started", run_id: "old", chat_id: "c", bot_id: "b1" },
    NOW - 60_000
  );
  assert.equal(t.calls.start.length, 0);
  assert.deepEqual(t.bridge.liveRuns(), []);
});

test("control_requested shows the target, posts proceed and ripples a click", async () => {
  const t = setup(true);
  const rect = { x: 100, y: 200, width: 40, height: 20 };
  await t.feed(request("r1", rect));
  assert.deepEqual(t.calls.begin, [{ rect, label: "Save", action: "click" }]);
  assert.deepEqual(t.calls.decide, [
    { id: "r1", decision: "proceed", reason: undefined }
  ]);
  assert.deepEqual(t.calls.clicks, [{ x: 120, y: 210 }]);
  // The request arrived while idle: the bridge started the run state first.
  assert.equal(t.calls.start.length, 1);
});

test("a refusal from the state machine is posted with a reason for the model", async () => {
  const t = setup(false);
  await t.bridge.onNativeState("human_driving");
  await t.feed(request("r2", null, "key"));
  assert.equal(t.calls.decide.length, 1);
  assert.equal(t.calls.decide[0].decision, "refuse");
  assert.match(t.calls.decide[0].reason ?? "", /taken control/);
  assert.equal(t.calls.begin[0].rect, null);
  assert.equal(t.calls.clicks.length, 0);
  assert.equal(t.calls.start.length, 0, "the human holds the wheel: no restart");
});

test("a request is answered once, and a stale one not at all", async () => {
  const t = setup();
  await t.feed(request("r3"));
  await t.feed(request("r3"));
  assert.equal(t.calls.decide.length, 1);
  t.advance(60_000);
  await t.feed(request("r4"), NOW + 1000);
  assert.equal(t.calls.decide.length, 1, "the daemon gave up on r4 long ago");
});

test("every native state change is reported to the daemon, and so is the heartbeat", async () => {
  const t = setup();
  await t.bridge.onNativeState("bot_driving");
  await t.bridge.onNativeState("human_driving");
  await t.bridge.heartbeat();
  assert.deepEqual(t.calls.put, [
    "bot_driving",
    "human_driving",
    "human_driving"
  ]);
});

test("only the user's explicit resume sends release:true, never a heartbeat or reopen", async () => {
  const t = setup();
  // Reopened UI: starts idle and heartbeats.
  await t.bridge.heartbeat();
  await t.bridge.onNativeState("human_driving");
  await t.bridge.heartbeat();
  await t.bridge.heartbeat();
  assert.deepEqual(t.calls.release, [false, false, false, false]);
  // The user hands the wheel back ("Bitirdim"): native state leaves human_driving.
  await t.bridge.onNativeState("idle");
  assert.equal(t.calls.release.at(-1), true);
  await t.bridge.heartbeat();
  assert.equal(t.calls.release.at(-1), false, "later heartbeats do not release");
  // Pause then resume to bot_driving.
  await t.bridge.onNativeState("paused");
  assert.equal(t.calls.release.at(-1), false);
  await t.bridge.onNativeState("bot_driving");
  assert.equal(t.calls.release.at(-1), true);
});

test("Stop (state goes idle while runs are live) cancels those runs", async () => {
  const t = setup();
  await t.feed({
    type: "run_started",
    run_id: "run-1",
    chat_id: "c",
    bot_id: "b1"
  });
  await t.feed({
    type: "run_started",
    run_id: "run-2",
    chat_id: "c",
    bot_id: "b1"
  });
  await t.bridge.onNativeState("bot_driving");
  await t.bridge.onNativeState("idle");
  assert.deepEqual(t.calls.cancel.sort(), ["run-1", "run-2"]);
  assert.deepEqual(t.bridge.liveRuns(), []);
  assert.equal(t.calls.put.at(-1), "idle");
  // idle again with nothing live cancels nothing more
  await t.bridge.onNativeState("idle");
  assert.equal(t.calls.cancel.length, 2);
});

test("a normal finish is not mistaken for Stop", async () => {
  const t = setup();
  await t.feed({
    type: "run_started",
    run_id: "run-1",
    chat_id: "c",
    bot_id: "b1"
  });
  await t.bridge.onNativeState("bot_driving");
  await t.feed({ type: "run_finished", run_id: "run-1", error: null });
  await t.bridge.onNativeState("idle"); // the event stopRun caused
  assert.deepEqual(t.calls.cancel, []);
});

test("refusal reasons name the state", () => {
  assert.match(refusalReason("human_driving"), /taken control/);
  assert.match(refusalReason("paused"), /paused/);
  assert.match(refusalReason("bot_driving"), /stopped/);
});

test("human_driving -> idle is a real Stop (stop_all) and cancels; give_back (-> bot_driving) does not", async () => {
  const t = setup();
  await t.feed({ type: "run_started", run_id: "run-live", chat_id: "c", bot_id: "b1" });
  await t.bridge.initializeNativeState("idle", 1);
  await t.bridge.onNativeState("human_driving", 1);
  await t.bridge.onNativeState("bot_driving", 1); // give_back: a hand-back
  assert.deepEqual(t.calls.cancel, []);
  assert.deepEqual(t.bridge.liveRuns(), ["run-live"]);
  await t.bridge.onNativeState("human_driving", 1);
  await t.bridge.onNativeState("idle", 2); // global Stop while the user drives
  assert.deepEqual(t.calls.cancel, ["run-live"]);
  assert.deepEqual(t.bridge.liveRuns(), []);
});

test("owned takeover: Bitirdim uses releaseControl, heartbeats never release, Stop uses override", async () => {
  const t = setup();
  await t.feed({
    type: "run_started",
    run_id: "run-1",
    chat_id: "c",
    bot_id: "b1"
  });
  await t.bridge.syncControlState({
    app_connected: true,
    expires_in_ms: 15_000,
    latched: "human_driving",
    mode: "human_driving",
    release: null,
    takeover: {
      id: "to-1",
      control_epoch: 3,
      input_expires_in_ms: 10_000,
      owner: "local-ui:1",
      session_id: "sess-1",
      state: "active",
      target_id: "target-1"
    }
  });
  t.bridge.setOwnedTakeoverId("to-1");
  await t.bridge.onNativeState("human_driving");
  await t.bridge.heartbeat();
  assert.equal(t.calls.release.some(Boolean), false);

  // Explicit Bitirdim release goes through releaseControl and keeps runs alive.
  const relReq: ReleaseRequest = {
    takeover_id: "to-1",
    control_epoch: 3,
    request_id: "req-rel-1",
    outcome: "completed",
    human_action: "login_completed"
  };
  await t.bridge.releaseOwnedTakeover(relReq);
  assert.equal(t.calls.releaseTakeover.length, 1);
  assert.equal(t.calls.releaseTakeover[0].request_id, "req-rel-1");
  assert.deepEqual(t.calls.cancel, []);
  assert.equal(t.bridge.takeover(), null);

  // A peer's takeover is not ours: a local Pause does not override on its behalf.
  await t.feed({
    type: "control_state_changed",
    state: "active",
    owner: "peer:phone",
    target_id: "target-1",
    reason: null
  });
  await t.bridge.onNativeState("paused");
  assert.equal(t.calls.override, 0);

  // Our own live takeover: a local Pause overrides instead of fighting the latch.
  await t.bridge.syncControlState({
    app_connected: true,
    expires_in_ms: 15_000,
    latched: "human_driving",
    mode: "human_driving",
    release: null,
    takeover: {
      id: "to-2",
      control_epoch: 4,
      input_expires_in_ms: 10_000,
      owner: "local-ui:1",
      session_id: "sess-1",
      state: "active",
      target_id: "target-1"
    }
  });
  t.bridge.setOwnedTakeoverId("to-2");
  await t.bridge.onNativeState("idle");
  await t.bridge.onNativeState("paused");
  assert.equal(t.calls.override, 1);

  // Explicit emergencyStop interrupts takeover and cancels live runs.
  await t.bridge.emergencyStop();
  assert.equal(t.calls.override, 2);
  assert.deepEqual(t.calls.cancel, ["run-1"]);
});

test("browser and remote targets NEVER touch local PC overlay/clicks and use panel overlay", async () => {
  let scope: TargetScope = "remote";
  const t = setup(true, () => scope);
  const rect = { x: 400, y: 300, width: 120, height: 44 };
  await t.feed({
    type: "run_started",
    run_id: "run-remote",
    chat_id: "c",
    bot_id: "b1"
  });
  await t.feed({
    type: "control_requested",
    request_id: "req-remote-1",
    run_id: "run-remote",
    bot_id: "b1",
    rect,
    label: "Submit order",
    action: "click"
  });
  assert.equal(t.calls.start.length, 0, "never opens local desktop run overlay");
  assert.equal(t.calls.begin.length, 0, "never highlights local PC desktop");
  assert.equal(t.calls.clicks.length, 0, "never clicks local PC desktop");
  assert.equal(t.calls.panelOverlay.length, 1);
  assert.deepEqual(t.calls.panelOverlay[0].rect, rect);
  assert.deepEqual(t.calls.decide, [
    { id: "req-remote-1", decision: "proceed", reason: undefined }
  ]);

  // Browser target while an owned takeover is paused refuses without touching native PC.
  scope = "browser";
  await t.feed({
    type: "control_state_changed",
    state: "paused",
    owner: "local-ui:1",
    target_id: "target-browser",
    reason: "disconnected"
  });
  await t.feed({
    type: "control_requested",
    request_id: "req-browser-2",
    run_id: "run-remote",
    bot_id: "b1",
    rect,
    label: "Next",
    action: "click"
  });
  assert.equal(t.calls.begin.length, 0);
  assert.equal(t.calls.clicks.length, 0);
  assert.equal(t.calls.decide[1].decision, "refuse");
});

test("resolveTargetScope distinguishes local_desktop from browser and remote targets", () => {
  const localDesktop = {
    target_id: "t-local",
    bot_id: null,
    run_id: null,
    host_id: "local",
    kind: "desktop" as const,
    generation: 1,
    capabilities: { codecs: ["jpeg" as const], view: true, input: true, reason: null },
    unavailable_reason: null
  };
  const remoteDesktop = {
    ...localDesktop,
    target_id: "t-remote",
    bot_id: "bot-r",
    run_id: "run-r",
    host_id: "peer:phone-or-mac",
    unavailable_reason: "remote_node_live_view" as const
  };
  const browser = {
    ...localDesktop,
    target_id: "t-browser",
    bot_id: "bot-b",
    run_id: "run-b",
    kind: "browser" as const
  };
  assert.equal(
    resolveTargetScope({
      runId: "run-r",
      botId: "bot-r",
      targets: [localDesktop, remoteDesktop, browser],
      selectedTarget: localDesktop,
      computerTarget: "this-pc"
    }),
    "remote"
  );
  assert.equal(
    resolveTargetScope({
      runId: "run-b",
      botId: "bot-b",
      targets: [localDesktop, remoteDesktop, browser],
      selectedTarget: localDesktop,
      computerTarget: "this-pc"
    }),
    "browser"
  );
  assert.equal(
    resolveTargetScope({
      runId: "run-other",
      botId: "bot-other",
      targets: [localDesktop],
      selectedTarget: localDesktop,
      computerTarget: "this-pc"
    }),
    "local_desktop"
  );
});

test("startRun preserves explicit botShape even when it differs from ID hash fallback", async () => {
  const calls: { bot: BotMeta; step: string }[] = [];
  const bridge = new ControlBridge({
    client: {
      listRuns: async () => [],
      putControlState: async () => undefined,
      decideControl: async () => undefined,
      cancelRun: async () => undefined
    },
    native: {
      beginAction: async () => true,
      startRun: async (bot, step) => void calls.push({ bot, step }),
      stopRun: async () => 1,
      click: async () => undefined
    },
    botMeta: (id) => ({
      botId: id,
      botName: "HeartBot",
      botShape: "heart",
      botColor: "#4c8dff"
    }),
    startingStep: "Working…",
    now: () => NOW
  });
  await bridge.handleEvent({
    v: 1,
    seq: 1,
    ts_ms: NOW,
    gseq: 1,
    event: {
      type: "run_started",
      run_id: "run-shape",
      chat_id: "c",
      bot_id: "bot-cube-0"
    }
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].bot.botId, "bot-cube-0");
  assert.equal(calls[0].bot.botShape, "heart");
});

test("reopening the app restores existing daemon runs so Stop cancels them", async () => {
  const t = setup();
  t.snapshot(async () => [{ id: "existing", bot_id: "b1", state: "running" }]);
  await t.feed({ type: "run_started", run_id: "existing", chat_id: "c", bot_id: "b1" }, NOW - 60_000);
  await t.bridge.restoreRuns();
  assert.deepEqual(t.bridge.liveRuns(), ["existing"]);
  assert.equal(t.calls.start.length, 1);
  await t.bridge.onNativeState("idle");
  assert.deepEqual(t.calls.cancel, ["existing"]);
});

test("restoration uses actual daemon states and does not release an existing human hold", async () => {
  const t = setup();
  await t.bridge.onNativeState("human_driving");
  t.snapshot(async () => [
    { id: "queued", bot_id: "b1", state: "queued" },
    { id: "waiting", bot_id: "b1", state: "waiting_approval" },
    { id: "paused", bot_id: "b1", state: "paused" },
    { id: "done", bot_id: "b1", state: "succeeded" },
    { id: "failed", bot_id: "b1", state: "failed" },
    { id: "cancelled", bot_id: "b1", state: "cancelled" }
  ]);
  await t.bridge.restoreRuns();
  assert.deepEqual(t.bridge.liveRuns().sort(), ["paused", "queued", "waiting"]);
  assert.equal(t.calls.start.length, 0);
  assert.equal(t.bridge.mode, "human_driving");
  assert.ok(t.calls.release.every((release) => !release));
});

test("a terminal event arriving during the initial snapshot wins over its stale row", async () => {
  const t = setup();
  let resolve!: (runs: Pick<Run, "id" | "bot_id" | "state">[]) => void;
  t.snapshot(() => new Promise((done) => { resolve = done; }));
  const restoring = t.bridge.restoreRuns();
  await t.feed({ type: "run_finished", run_id: "finished-during-read", error: null });
  resolve?.([{ id: "finished-during-read", bot_id: "b1", state: "running" }]);
  await restoring;
  assert.deepEqual(t.bridge.liveRuns(), []);
  assert.equal(t.calls.start.length, 0);
});

test("fresh starts received while the snapshot is pending stay live even when absent from it", async () => {
  const t = setup();
  let resolve!: (runs: Pick<Run, "id" | "bot_id" | "state">[]) => void;
  t.snapshot(() => new Promise((done) => { resolve = done; }));
  const restoring = t.bridge.restoreRuns();
  await t.feed({ type: "run_started", run_id: "new", bot_id: "b1", chat_id: "c" });
  resolve?.([]);
  await restoring;
  assert.deepEqual(t.bridge.liveRuns(), ["new"]);
  await t.bridge.onNativeState("idle");
  assert.deepEqual(t.calls.cancel, ["new"]);
});

test("Stop during restoration cancels snapshot runs without starting the native widget again", async () => {
  const t = setup();
  let resolve!: (runs: Pick<Run, "id" | "bot_id" | "state">[]) => void;
  t.snapshot(() => new Promise((done) => { resolve = done; }));
  const restoring = t.bridge.restoreRuns();
  await t.feed({ type: "run_started", run_id: "new", bot_id: "b1", chat_id: "c" });
  await t.bridge.onNativeState("idle");
  resolve?.([{ id: "existing", bot_id: "b1", state: "running" }]);
  await restoring;
  assert.deepEqual(t.calls.cancel.sort(), ["existing", "new"]);
  assert.deepEqual(t.bridge.liveRuns(), []);
  assert.equal(t.calls.start.length, 1);
});

test("disposing the old connection drops its active runs and prevents late cancellation", async () => {
  const old = setup();
  await old.feed({ type: "run_started", run_id: "old-daemon", bot_id: "b1", chat_id: "c" });
  old.bridge.dispose();
  const current = setup();
  current.snapshot(async () => [{ id: "current-daemon", bot_id: "b1", state: "running" }]);
  await current.bridge.restoreRuns();
  await old.bridge.onNativeState("idle");
  await current.bridge.onNativeState("idle");
  assert.deepEqual(old.bridge.liveRuns(), []);
  assert.deepEqual(old.calls.cancel, []);
  assert.deepEqual(current.calls.cancel, ["current-daemon"]);
});

test("a delayed snapshot cannot revive an already disposed daemon connection", async () => {
  const old = setup();
  let resolve!: (runs: Pick<Run, "id" | "bot_id" | "state">[]) => void;
  old.snapshot(() => new Promise((done) => { resolve = done; }));
  const restoring = old.bridge.restoreRuns();
  old.bridge.dispose();
  resolve?.([{ id: "old-daemon", bot_id: "b1", state: "running" }]);
  await restoring;
  assert.deepEqual(old.bridge.liveRuns(), []);
  assert.equal(old.calls.start.length, 0);
  assert.deepEqual(old.calls.cancel, []);
});

test("a failed initial run snapshot can be retried", async () => {
  const t = setup();
  t.snapshot(async () => { throw new Error("temporarily offline"); });
  await t.bridge.restoreRuns();
  t.snapshot(async () => [{ id: "existing", bot_id: "b1", state: "running" }]);
  await t.bridge.restoreRuns();
  assert.deepEqual(t.bridge.liveRuns(), ["existing"]);
});

test("a stale initial idle snapshot does not stop a freshly started run", async () => {
  const t = setup();
  await t.feed({ type: "run_started", run_id: "new", chat_id: "c", bot_id: "b1" });
  await t.bridge.initializeNativeState("idle");
  assert.equal(t.bridge.mode, "bot_driving");
  assert.deepEqual(t.bridge.liveRuns(), ["new"]);
  assert.deepEqual(t.calls.cancel, []);
  assert.ok(t.calls.release.every((release) => !release));
});

test("restoring only paused runs does not make the bot start processing", async () => {
  const t = setup();
  t.snapshot(async () => [{ id: "paused", bot_id: "b1", state: "paused" }]);
  await t.bridge.restoreRuns();
  assert.deepEqual(t.bridge.liveRuns(), ["paused"]);
  assert.equal(t.bridge.mode, "idle");
  assert.equal(t.calls.start.length, 0);
});

test("explicit Stop from native idle wins over the first in-flight snapshot", async () => {
  const t = setup();
  let resolve!: (runs: Pick<Run, "id" | "bot_id" | "state">[]) => void;
  t.snapshot(() => new Promise((done) => { resolve = done; }));
  const restoring = t.bridge.restoreRuns();
  await t.bridge.onNativeState("idle");
  resolve([{ id: "existing", bot_id: "b1", state: "running" }]);
  await restoring;
  assert.deepEqual(t.calls.cancel, ["existing"]);
  assert.equal(t.calls.start.length, 0);
  assert.deepEqual(t.bridge.liveRuns(), []);
});

test("Stop intent survives a failed first snapshot until a successful retry", async () => {
  const t = setup();
  let reject!: (error: Error) => void;
  t.snapshot(() => new Promise((_, fail) => { reject = fail; }));
  const restoring = t.bridge.restoreRuns();
  await t.bridge.onNativeState("idle");
  reject(new Error("offline"));
  await restoring;
  t.snapshot(async () => [{ id: "existing", bot_id: "b1", state: "running" }]);
  await t.bridge.restoreRuns();
  assert.deepEqual(t.calls.cancel, ["existing"]);
  assert.equal(t.calls.start.length, 0);
  assert.deepEqual(t.bridge.liveRuns(), []);
});

test("an automatic finish-to-idle event cannot cancel other runs in the pending snapshot", async () => {
  const t = setup();
  let resolve!: (runs: Pick<Run, "id" | "bot_id" | "state">[]) => void;
  t.snapshot(() => new Promise((done) => { resolve = done; }));
  const restoring = t.bridge.restoreRuns();
  await t.feed({ type: "run_started", run_id: "new", bot_id: "b1", chat_id: "c" });
  await t.feed({ type: "run_finished", run_id: "new", error: null });
  await t.bridge.onNativeState("idle", t.calls.stop);
  resolve([{ id: "other", bot_id: "b1", state: "running" }]);
  await restoring;
  assert.deepEqual(t.bridge.liveRuns(), ["other"]);
  assert.deepEqual(t.calls.cancel, []);
  assert.equal(t.calls.start.length, 2);
});

// ---- review of PR 56: bridge lifecycle, release failure, ownership by id ----

function idle(): ControlStateDto {
  return {
    app_connected: true,
    expires_in_ms: 15_000,
    latched: null,
    mode: "idle",
    release: null,
    takeover: null
  };
}

function owned(id: string, state = "active"): ControlStateDto {
  return {
    ...idle(),
    latched: "human_driving",
    mode: "human_driving",
    takeover: {
      id,
      control_epoch: 3,
      input_expires_in_ms: 10_000,
      owner: "local-ui:1",
      session_id: "sess-1",
      state: state as "active",
      target_id: "target-1"
    }
  };
}

function bareBridge(client: Partial<ConstructorParameters<typeof ControlBridge>[0]["client"]> = {}) {
  const log = { cancel: [] as string[], put: [] as boolean[], override: 0 };
  const bridge = new ControlBridge({
    client: {
      listRuns: async () => [],
      putControlState: async (_m, release) => void log.put.push(release === true),
      decideControl: async () => undefined,
      cancelRun: async (id) => void log.cancel.push(id),
      overrideControl: async () => {
        log.override += 1;
        return idle();
      },
      ...client
    },
    native: {
      beginAction: async () => true,
      startRun: async () => undefined,
      stopRun: async () => 0,
      click: async () => undefined
    },
    botMeta: (id) => ({ botId: id, botName: id }),
    startingStep: "x",
    now: () => NOW
  });
  const feed = (event: DaemonEvent) =>
    bridge.handleEvent({ v: 1, seq: 1, ts_ms: NOW, gseq: 1, event });
  return { bridge, log, feed };
}

test("control-state refreshes keep the same bridge: its live runs survive and Stop still cancels them", async () => {
  const { bridge, log, feed } = bareBridge();
  let latest: ControlStateDto | null = null;
  const registry = createBridgeRegistry(() => latest);
  registry.register(bridge);
  await feed({ type: "run_started", run_id: "run-1", chat_id: "c", bot_id: "b1" });
  assert.deepEqual(bridge.liveRuns(), ["run-1"]);

  // many refreshes (viewer open, control_state_changed, takeover, release ...)
  for (let i = 0; i < 4; i += 1) {
    latest = i % 2 ? idle() : owned("to-1");
    registry.syncControlState(latest);
    registry.setOwnedTakeoverId(i % 2 ? null : "to-1");
  }
  await bridge.syncControlState(idle());
  assert.equal(registry.current(), bridge, "same bridge instance");
  assert.deepEqual(bridge.liveRuns(), ["run-1"], "live runs not reset");

  await registry.current()?.emergencyStop();
  assert.deepEqual(log.cancel, ["run-1"], "Stop cancels the run");
});

test("registering a bridge hands it the current snapshot and owned takeover id", async () => {
  const { bridge, log } = bareBridge();
  const registry = createBridgeRegistry(() => owned("to-9"));
  registry.setOwnedTakeoverId("to-9");
  registry.register(bridge);
  await bridge.syncControlState(owned("to-9"));
  await bridge.heartbeat(true);
  assert.deepEqual(log.put, [false], "owned takeover blocks the legacy release");
});

test("a failed release rejects (not null) and leaves the bridge usable", async () => {
  let fail = true;
  const { bridge } = bareBridge({
    releaseControl: async () => {
      if (fail) {
        throw new Error("409");
      }
      return idle();
    }
  });
  const req: ReleaseRequest = {
    takeover_id: "to-1",
    control_epoch: 3,
    request_id: "req-1",
    outcome: "completed"
  };
  await assert.rejects(() => bridge.releaseOwnedTakeover(req), /409/);
  fail = false;
  const dto = await bridge.releaseOwnedTakeover(req);
  assert.equal(dto?.takeover, null, "chain still works after a failure");
});

test("legacy release is blocked only for OUR live takeover, not a peer's or a paused one", async () => {
  const { bridge, log } = bareBridge();
  bridge.setOwnedTakeoverId("to-mine");

  await bridge.syncControlState(owned("to-peer"));
  await bridge.heartbeat(true);
  assert.deepEqual(log.put.at(-1), true, "a peer's takeover does not block");

  await bridge.syncControlState(owned("to-mine", "paused"));
  await bridge.heartbeat(true);
  assert.deepEqual(log.put.at(-1), true, "a paused takeover does not block");

  await bridge.syncControlState(owned("to-mine"));
  await bridge.heartbeat(true);
  assert.deepEqual(log.put.at(-1), false, "our live takeover blocks");
});

test("a missed control_state_changed 'released' event is healed from GET /control/state", async () => {
  let snapshot: ControlStateDto = owned("to-mine");
  const { bridge, log } = bareBridge({
    getControlState: async () => snapshot
  });
  bridge.setOwnedTakeoverId("to-mine");
  await bridge.refreshControlState();
  await bridge.heartbeat(true);
  assert.equal(log.put.at(-1), false);

  // the release event was never delivered; the daemon snapshot says it is gone
  snapshot = idle();
  await bridge.refreshControlState();
  await bridge.heartbeat(true);
  assert.equal(log.put.at(-1), true, "no stale tracker blocks Resume");
});

test("control_state_changed events re-read the snapshot (events carry no takeover id)", async () => {
  const { bridge, log, feed } = bareBridge({
    getControlState: async () => owned("to-mine")
  });
  bridge.setOwnedTakeoverId("to-mine");
  await feed({
    type: "control_state_changed",
    state: "active",
    owner: "local-ui:1",
    target_id: "target-1",
    reason: null
  });
  assert.equal(bridge.takeover()?.id, "to-mine");
  await bridge.heartbeat(true);
  assert.equal(log.put.at(-1), false);
});

// ---- reconciled with PR 56: restored runs + Stop invariants meet takeover semantics ----

test("a give_back during a pending restore is not a Stop for the restored runs; a stop_all from human_driving is", async () => {
  const t = setup();
  await t.bridge.initializeNativeState("human_driving", 1);
  let resolve!: (rows: { id: string; bot_id: string; state: string }[]) => void;
  t.snapshot(() => new Promise((r) => { resolve = r; }));
  const restoring = t.bridge.restoreRuns();
  await t.bridge.onNativeState("bot_driving", 1); // give_back while the run list loads
  resolve([{ id: "live", bot_id: "b1", state: "running" }]);
  await restoring;
  assert.deepEqual(t.calls.cancel, [], "hand-back never cancels");
  assert.deepEqual(t.bridge.liveRuns(), ["live"]);

  const u = setup();
  await u.bridge.initializeNativeState("human_driving", 1);
  let resolve2!: (rows: { id: string; bot_id: string; state: string }[]) => void;
  u.snapshot(() => new Promise((r) => { resolve2 = r; }));
  const restoring2 = u.bridge.restoreRuns();
  await u.bridge.onNativeState("idle", 2); // Stop hotkey while the user drives
  resolve2([{ id: "live", bot_id: "b1", state: "running" }]);
  await restoring2;
  assert.deepEqual(u.calls.cancel, ["live"]);
});

test("an explicit emergency Stop covers runs a pending restore finds later", async () => {
  const t = setup();
  let resolve!: (rows: { id: string; bot_id: string; state: string }[]) => void;
  t.snapshot(() => new Promise((r) => { resolve = r; }));
  const restoring = t.bridge.restoreRuns();
  await t.bridge.emergencyStop();
  resolve([{ id: "late", bot_id: "b1", state: "running" }]);
  await restoring;
  assert.deepEqual(t.calls.cancel, ["late"]);
});

test("restored runs of a browser/remote target never open the local desktop overlay", async () => {
  const t = setup(true, () => "browser");
  t.snapshot(async () => [{ id: "web", bot_id: "b1", state: "running" }]);
  await t.bridge.restoreRuns();
  assert.deepEqual(t.bridge.liveRuns(), ["web"], "still cancellable by Stop");
  assert.equal(t.calls.start.length, 0, "no local overlay for a browser target");
});

test("restored runs survive control-state refreshes through the stable registry and Stop still cancels them", async () => {
  const t = setup();
  t.snapshot(async () => [{ id: "existing", bot_id: "b1", state: "running" }]);
  await t.bridge.restoreRuns();
  const registry = createBridgeRegistry(() => null);
  registry.register(t.bridge);
  for (let i = 0; i < 3; i += 1) {
    registry.syncControlState({
      app_connected: true,
      expires_in_ms: 15_000,
      latched: null,
      mode: "idle",
      release: null,
      takeover: null
    });
  }
  assert.equal(registry.current(), t.bridge);
  assert.deepEqual(t.bridge.liveRuns(), ["existing"]);
  await t.bridge.emergencyStop();
  assert.deepEqual(t.calls.cancel, ["existing"]);
});

// ---- review of #62 after #56: what native idle means ---------------------------

const browserTarget = {
  target_id: "tb",
  bot_id: null,
  run_id: "web",
  host_id: "local",
  kind: "browser" as const,
  generation: 1,
  capabilities: { codecs: ["jpeg" as const], view: true, input: true, reason: null },
  unavailable_reason: null
};

test("a same-generation idle (hotkey applied/edited, step, overlay clear) is not a Stop, also while a restore is pending", async () => {
  const t = setup();
  await t.bridge.initializeNativeState("idle", 3);
  let resolve!: (rows: { id: string; bot_id: string; state: string }[]) => void;
  t.snapshot(() => new Promise((r) => { resolve = r; }));
  const restoring = t.bridge.restoreRuns();
  await t.bridge.onNativeState("idle", 3); // set_stop_hotkey at app launch
  resolve([
    { id: "queued", bot_id: "b1", state: "queued" },
    { id: "running", bot_id: "b1", state: "running" },
    { id: "paused", bot_id: "b1", state: "paused" }
  ]);
  await restoring;
  assert.deepEqual(t.calls.cancel, [], "nothing cancelled");
  assert.deepEqual(t.bridge.liveRuns().sort(), ["paused", "queued", "running"]);
  // a later hotkey edit while runs are tracked: still not a Stop
  await t.bridge.onNativeState("idle", 3);
  assert.deepEqual(t.calls.cancel, []);
  // stop_all raises the generation: now it is
  await t.bridge.onNativeState("idle", 4);
  assert.deepEqual(t.calls.cancel.sort(), ["paused", "queued", "running"]);
});

test("a Stop survives a bridge swap: it cancels the runs and the new bridge does not re-track them", async () => {
  const cancelled = new Set<string>();
  const old = setup(true, undefined, { cancelled });
  await old.feed({ type: "run_started", run_id: "r1", chat_id: "c", bot_id: "b1" });
  await old.bridge.initializeNativeState("bot_driving", 1);
  void old.bridge.onNativeState("idle", 2); // Stop, queued on the old bridge
  old.bridge.dispose(); // the daemon client is replaced before it runs
  await old.bridge.drain();
  assert.deepEqual(old.calls.cancel, ["r1"], "the Stop still cancels");

  const fresh = setup(true, undefined, { cancelled });
  fresh.snapshot(async () => [{ id: "r1", bot_id: "b1", state: "running" }]);
  await fresh.bridge.restoreRuns();
  assert.deepEqual(fresh.bridge.liveRuns(), [], "not re-tracked");
  assert.deepEqual(fresh.calls.cancel, []);
});

test("restored runs resolve their scope by run id; a browser run with no bot id never opens the local overlay", async () => {
  const seen: { runId?: string; botId?: string }[] = [];
  const t = setup(true, (info) => {
    seen.push(info);
    return resolveTargetScope({
      ...info,
      targets: [browserTarget],
      selectedTarget: null,
      computerTarget: "this-pc"
    });
  });
  t.snapshot(async () => [{ id: "web", bot_id: "other-bot", state: "running" }]);
  await t.bridge.restoreRuns();
  assert.ok(seen.some((i) => i.runId === "web"), "run id passed");
  assert.equal(t.calls.start.length, 0);
  assert.deepEqual(t.bridge.liveRuns(), ["web"]);
});

test("every restored local run is driven, not only the first processing row", async () => {
  const t = setup(true, ({ runId }) => (runId === "web" ? "browser" : "local_desktop"));
  t.snapshot(async () => [
    { id: "web", bot_id: "b1", state: "running" },
    { id: "loc", bot_id: "b2", state: "running" }
  ]);
  await t.bridge.restoreRuns();
  assert.equal(t.calls.start.length, 1);
  assert.equal(t.calls.start[0].bot.botId, "b2");
});

test("a restored run waits for the targets instead of defaulting to the local overlay", async () => {
  let loaded = false;
  const t = setup(true, () => "local_desktop", { targetsLoaded: () => loaded });
  t.snapshot(async () => [{ id: "x", bot_id: "b1", state: "running" }]);
  await t.bridge.restoreRuns();
  assert.equal(t.calls.start.length, 0, "targets not loaded yet");
  assert.deepEqual(t.bridge.liveRuns(), ["x"], "still cancellable");
  loaded = true;
  await t.bridge.retryDeferredStarts();
  assert.equal(t.calls.start.length, 1);
});

// ---- the table: every way native state becomes idle -> are runs cancelled? ----

interface IdleRow {
  name: string;
  act: (t: ReturnType<typeof setup>) => Promise<void>;
  cancelled: boolean;
}
const idleRows: IdleRow[] = [
  { name: "global/widget/banner Stop hotkey from bot_driving", act: async (t) => { await t.bridge.onNativeState("bot_driving", 1); await t.bridge.onNativeState("idle", 2); }, cancelled: true },
  { name: "Stop while the user drives (human_driving -> idle)", act: async (t) => { await t.bridge.onNativeState("human_driving", 1); await t.bridge.onNativeState("idle", 2); }, cancelled: true },
  { name: "Stop from paused", act: async (t) => { await t.bridge.onNativeState("paused", 1); await t.bridge.onNativeState("idle", 2); }, cancelled: true },
  { name: "Stop while already idle (stop_all, new generation)", act: async (t) => { await t.bridge.onNativeState("idle", 2); }, cancelled: true },
  { name: "emergency Stop (viewer / banner with a takeover)", act: async (t) => { await t.bridge.emergencyStop(); }, cancelled: true },
  { name: "hotkey applied at app launch / edited (same generation)", act: async (t) => { await t.bridge.initializeNativeState("idle", 1); await t.bridge.onNativeState("idle", 1); }, cancelled: false },
  { name: "step text / overlay clear / refused action (same generation)", act: async (t) => { await t.bridge.initializeNativeState("idle", 1); await t.bridge.onNativeState("idle", 1); await t.bridge.onNativeState("idle", 1); }, cancelled: false },
  { name: "app start: initial idle snapshot", act: async (t) => { await t.bridge.initializeNativeState("idle", 1); }, cancelled: false },
  { name: "startup race: buffered same-generation idle replayed after the snapshot", act: async (t) => { await t.bridge.initializeNativeState("idle", 1); await t.bridge.onNativeState("idle", 1); }, cancelled: false },
  { name: "Bitirdim: give_back (human_driving -> bot_driving)", act: async (t) => { await t.bridge.onNativeState("human_driving", 1); await t.bridge.onNativeState("bot_driving", 1); }, cancelled: false },
  { name: "Bitirdim: release in flight / failed (releaseOwnedTakeover)", act: async (t) => {
      await t.bridge.releaseOwnedTakeover({ takeover_id: "to", control_epoch: 1, request_id: "r", outcome: "completed" }).catch(() => undefined);
    }, cancelled: false },
  { name: "takeover expired / daemon pause (snapshot only, no native idle)", act: async (t) => {
      await t.bridge.syncControlState({ app_connected: true, expires_in_ms: 1, latched: "paused", mode: "paused", release: null, takeover: null });
    }, cancelled: false },
  { name: "daemon restart / client replaced: the new bridge restores", act: async (t) => {
      t.snapshot(async () => [{ id: "r", bot_id: "b1", state: "running" }]);
      await t.bridge.restoreRuns();
    }, cancelled: false }
];
// Tray Stop All is not a native idle: `cancelAllRuns` (runState.test.ts) cancels
// every cancellable daemon run. The echo of a bridge-caused finish is pinned by
// "a normal finish is not mistaken for Stop" and the generation test above.

for (const row of idleRows) {
  test(`idle table: ${row.name} -> runs ${row.cancelled ? "cancelled" : "kept"}`, async () => {
    const t = setup();
    await t.feed({ type: "run_started", run_id: "r", chat_id: "c", bot_id: "b1" });
    await row.act(t);
    assert.deepEqual(t.calls.cancel, row.cancelled ? ["r"] : []);
  });
}

// ---- second look at #62: startup buffer, unknown scope, hung client / failed cancel ----

test("a Stop already contained in the initial snapshot is recognised via the buffered earlier generation", async () => {
  // opening against a live daemon (bot_driving, gen 5); the user hits Stop (idle, gen 6)
  // while getControlState is in flight, and the snapshot already carries gen 6
  const plan = planNativeReplay(
    { mode: "idle", generation: 6 },
    [
      { mode: "bot_driving", generation: 5 },
      { mode: "idle", generation: 6 }
    ]
  );
  assert.equal(plan.baseline, 5, "walk from the lower buffered generation");
  assert.deepEqual(plan.replay, [{ mode: "idle", generation: 6 }], "older event not applied");

  const t = setup();
  await t.feed({ type: "run_started", run_id: "r", chat_id: "c", bot_id: "b1" });
  await t.bridge.initializeNativeState("idle", plan.baseline);
  for (const ev of plan.replay) await t.bridge.onNativeState(ev.mode, ev.generation);
  assert.deepEqual(t.calls.cancel, ["r"], "the Stop cancels");
  assert.equal(t.bridge.mode, "idle", "the older bot_driving was not re-applied");

  // no lower generation buffered: indistinguishable from "already idle" (not a Stop)
  const plain = planNativeReplay({ mode: "idle", generation: 6 }, [{ mode: "idle", generation: 6 }]);
  assert.equal(plain.baseline, 6);
});

test("a Stop flushed while the host is torn down during init reaches the next bridge", async () => {
  const carry: { stopAt: number | null } = { stopAt: null };
  const cancelled = new Set<string>();
  const old = setup(true, undefined, { carry, cancelled });
  const plan = planNativeReplay(
    { mode: "idle", generation: 6 },
    [{ mode: "bot_driving", generation: 5 }, { mode: "idle", generation: 6 }]
  );
  await old.bridge.initializeNativeState("idle", plan.baseline);
  for (const ev of plan.replay) void old.bridge.onNativeState(ev.mode, ev.generation);
  old.bridge.dispose(); // cleanup right after the flush
  await old.bridge.drain();
  assert.notEqual(carry.stopAt, null, "the Stop is carried");

  const fresh = setup(true, undefined, { carry, cancelled });
  fresh.snapshot(async () => [{ id: "live", bot_id: "b1", state: "running" }]);
  await fresh.bridge.restoreRuns();
  assert.deepEqual(fresh.calls.cancel, ["live"], "the next bridge cancels what it finds");
  assert.equal(carry.stopAt, null, "consumed once restored");
});

test("unknown scope never becomes the local overlay: no startRun before targets and settings settled", async () => {
  let settled = false;
  const t = setup(true, () => "local_desktop", { targetsLoaded: () => settled });
  t.snapshot(async () => [{ id: "web", bot_id: "other", state: "running" }]);
  await t.bridge.restoreRuns();
  for (let i = 0; i < 6; i += 1) await t.bridge.retryDeferredStarts(); // far past the old 3-try give-up
  assert.equal(t.calls.start.length, 0, "still waiting");
  assert.deepEqual(t.bridge.liveRuns(), ["web"], "but cancellable");

  // a settled fetch that lacks the run keeps the resolver's fallback
  settled = true;
  await t.bridge.retryDeferredStarts();
  assert.equal(t.calls.start.length, 1);
});

test("a settled fetch with the browser target present still keeps the local overlay closed", async () => {
  const t = setup(
    true,
    (info) =>
      resolveTargetScope({
        ...info,
        targets: [browserTarget],
        selectedTarget: null,
        computerTarget: "this-pc"
      }),
    { targetsLoaded: () => true }
  );
  t.snapshot(async () => [{ id: "web", bot_id: "other", state: "running" }]);
  await t.bridge.restoreRuns();
  await t.bridge.retryDeferredStarts();
  assert.equal(t.calls.start.length, 0);
});

test("the shared cancelled set is filled at the Stop decision, before the heartbeat completes", async () => {
  const cancelled = new Set<string>();
  const t = setup(true, undefined, { cancelled });
  await t.feed({ type: "run_started", run_id: "r1", chat_id: "c", bot_id: "b1" });
  let release!: () => void;
  t.hold.current = new Promise<void>((r) => { release = r; }); // the PUT /control/state hangs
  const stopping = t.bridge.onNativeState("idle", 2);
  await new Promise((r) => setTimeout(r, 5));
  assert.ok(cancelled.has("r1"), "recorded while the heartbeat is still pending");
  release();
  await stopping;
});

test("a bounded wait lets the next bridge restore although the retired client hangs", async () => {
  const never = new Promise<void>(() => undefined);
  const started = Date.now();
  await waitBounded(never, 20);
  assert.ok(Date.now() - started < 1000);
  await waitBounded(Promise.reject(new Error("x")), 1000); // a rejection also releases the wait
});

test("a cancel that rejects is not recorded: the run is tracked again and a later bridge tracks it too", async () => {
  const cancelled = new Set<string>();
  const old = setup(true, undefined, { cancelled });
  await old.feed({ type: "run_started", run_id: "r1", chat_id: "c", bot_id: "b1" });
  old.cancelFail.current = true;
  await old.bridge.onNativeState("idle", 2);
  assert.ok(!cancelled.has("r1"), "not suppressed");
  assert.deepEqual(old.bridge.liveRuns(), ["r1"], "tracked again so Stop can retry");
  old.cancelFail.current = false;
  await old.bridge.onNativeState("idle", 3);
  assert.deepEqual(old.calls.cancel, ["r1"], "the retry cancels it");

  const other = setup(true, undefined, { cancelled: new Set<string>() });
  other.cancelFail.current = true;
  const swapped = new Set<string>();
  const a = setup(true, undefined, { cancelled: swapped });
  await a.feed({ type: "run_started", run_id: "r2", chat_id: "c", bot_id: "b1" });
  a.cancelFail.current = true;
  await a.bridge.onNativeState("idle", 2);
  const next = setup(true, undefined, { cancelled: swapped });
  next.snapshot(async () => [{ id: "r2", bot_id: "b1", state: "running" }]);
  await next.bridge.restoreRuns();
  assert.deepEqual(next.bridge.liveRuns(), ["r2"], "a later bridge still tracks the live run");
  void other;
});
