import assert from "node:assert/strict";
import { test } from "node:test";
import { formatElapsed, runBarModel, runBarVisible, singleRunStop } from "./runBar.ts";

test("one bar: absent when idle, one model per situation", () => {
  assert.equal(runBarModel({ chat: "none", native: "idle" }), null);
  const chatOnly = runBarModel({ chat: "live", native: "idle" });
  assert.equal(chatOnly?.kind, "running");
  assert.equal(chatOnly?.canPause, false, "no desktop run, nothing to pause");
  assert.equal(chatOnly?.canBigView, false);
  const driving = runBarModel({ chat: "live", native: "bot_driving" });
  assert.deepEqual(
    [driving?.canPause, driving?.canWheel, driving?.canBigView],
    [true, true, true]
  );
  assert.equal(runBarModel({ chat: "waiting_approval", native: "idle" })?.kind, "waiting");
  assert.equal(runBarModel({ chat: "paused", native: "idle" })?.canResume, true);
  assert.equal(runBarModel({ chat: "none", native: "human_driving" })?.kind, "human");
});

test("a Stop status keeps the bar even after the run ended", () => {
  assert.equal(runBarVisible(null, false), false);
  assert.equal(runBarVisible(null, true), true);
});

test("elapsed time formatting", () => {
  assert.equal(formatElapsed(7_400), "0:07");
  assert.equal(formatElapsed(750_000), "12:30");
  assert.equal(formatElapsed(3_723_000), "1:02:03");
});

test("the bar shows Stop progress of a single chat run from its ledger phase", () => {
  assert.equal(singleRunStop("tracked"), null);
  assert.equal(singleRunStop("held"), "stopping");
  assert.equal(singleRunStop("cancel_requested"), "stopping");
  assert.equal(singleRunStop("failed"), "failed");
});
