import assert from "node:assert/strict";
import { test } from "node:test";
import type { ControlModeName } from "../api/types.ts";
import { FakeDaemon, FakeNative, flush, makeLedger } from "./ledgerFakes.ts";
import {
  overrideOptionsFor,
  restoreRuns,
  routeNativeSnapshot,
  stopTransport
} from "./stopHost.ts";
import type { StopOrigin } from "./runLedger.ts";

function sink() {
  const modes: [ControlModeName, number | undefined][] = [];
  let restored = 0;
  return {
    modes,
    get restored() {
      return restored;
    },
    onNativeState: async (mode: ControlModeName, generation?: number) => {
      modes.push([mode, generation]);
    },
    restoreDriving: async () => {
      restored += 1;
    }
  };
}

test("native snapshots reach the ledger first; the bridge only gets modes newer than the last (stateRev)", () => {
  const { ledger, native } = makeLedger();
  const bridge = sink();
  native.mode = "bot_driving";
  const a = native.touch();
  native.mode = "human_driving";
  const b = native.touch();
  routeNativeSnapshot(ledger, bridge, { ...b, mode: "human_driving" });
  routeNativeSnapshot(ledger, bridge, { ...a, mode: "bot_driving" }); // delivered late
  assert.deepEqual(bridge.modes.map(([m]) => m), ["human_driving"], "the older mode is not applied");
  // a user stop arriving through the same path is a Stop record even without a bridge
  const stop = native.userStop(null);
  routeNativeSnapshot(ledger, null, { ...stop, mode: "idle" });
  assert.equal(ledger.stopRecords().length, 1);
});

test("restoreRuns applies the client's list and drives restored runs only for the current client", async () => {
  const { ledger } = makeLedger();
  ledger.onNativeSnapshot(new FakeNative().snapshot());
  const daemon = new FakeDaemon();
  daemon.start("r1", "running");
  const old = ledger.attachClient(daemon.transport());
  const bridge = sink();
  let release: () => void = () => {};
  const slow = {
    listRuns: () =>
      new Promise<ReturnType<FakeDaemon["list"]>>((resolve) => {
        release = () => resolve(daemon.list());
      })
  };
  const pending = restoreRuns(ledger, old, slow, bridge);
  ledger.attachClient(daemon.transport()); // swapped while the list was in flight
  release();
  await pending;
  assert.equal(ledger.run("r1"), undefined, "a stale list adds nothing");
  assert.equal(bridge.restored, 0, "and drives nothing");

  const current = ledger.client();
  assert.ok(current);
  await restoreRuns(ledger, current, { listRuns: async () => daemon.list() }, bridge);
  assert.equal(ledger.run("r1")?.phase, "tracked");
  assert.equal(bridge.restored, 1);
  assert.equal(current.synced, true);
});

test("stopTransport passes the body and the attempt signal through to the client", async () => {
  const seen: unknown[] = [];
  const transport = stopTransport({
    stopRuns: async (body, signal) => {
      seen.push(body, signal);
      return { stop_id: body.stop_id, first_seen: true, results: [] };
    }
  });
  const signal = new AbortController().signal;
  await transport.stopRuns({ stop_id: "x", scope: "listed", run_ids: [] }, signal);
  assert.deepEqual(seen, [{ stop_id: "x", scope: "listed", run_ids: [] }, signal]);
});

// Entry-point table: every local entry point is ONE Stop with the same three effects.
test("entry points: hotkey/banner/viewer go through stopAll; widget/tray/global hotkey through native stopSeq", async () => {
  const rows: { name: string; press: (w: ReturnType<typeof makeLedger>) => void; origin: StopOrigin; token: boolean }[] = [
    { name: "in-window hotkey", press: (w) => void w.ledger.stopAll("hotkey"), origin: "hotkey", token: true },
    { name: "run banner", press: (w) => void w.ledger.stopAll("banner"), origin: "banner", token: true },
    { name: "live viewer", press: (w) => void w.ledger.stopAll("viewer"), origin: "viewer", token: true },
    {
      name: "widget / tray / global hotkey (native user stop, no token)",
      press: (w) => {
        const snap = w.native.userStop(null);
        w.ledger.onNativeSnapshot(snap);
      },
      origin: "native",
      token: false
    }
  ];
  for (const mode of ["bot_driving", "human_driving", "paused", "idle"] as const) {
    for (const row of rows) {
      const w = makeLedger();
      w.native.mode = mode;
      w.ledger.onNativeSnapshot(w.native.snapshot());
      const daemon = new FakeDaemon();
      daemon.start("r1", "paused");
      const h = w.ledger.attachClient(daemon.transport());
      w.ledger.beginList(h);
      w.ledger.applyList(h, daemon.list());
      const overrides: boolean[] = [];
      w.ledger.onStop((rec) => overrides.push(overrideOptionsFor(rec).onlyIfControlled));
      row.press(w);
      const label = `${row.name} @ ${mode}`;
      const recs = w.ledger.stopRecords();
      assert.equal(recs.length, 1, label);
      assert.equal(recs[0]?.origin, row.origin, label);
      assert.equal(w.native.mode, "idle", `${label}: native halted`);
      assert.equal(w.native.tokens.length, 1, `${label}: exactly one native stop`);
      assert.equal(w.native.tokens[0] === recs[0]?.id, row.token, `${label}: token`);
      assert.equal(daemon.requests.length, 1, `${label}: one daemon request`);
      assert.equal(daemon.requests[0]?.body.stop_id, recs[0]?.id, label);
      assert.equal(w.ledger.mayAct("r1"), false, `${label}: run held`);
      assert.deepEqual(overrides, [row.origin !== "viewer"], `${label}: takeover override once`);
      daemon.deliver(0);
      await flush();
      assert.equal(w.ledger.stopRecords()[0]?.phase, "settled", label);
    }
  }
});
