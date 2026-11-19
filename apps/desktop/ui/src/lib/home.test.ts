import assert from "node:assert/strict";
import { test } from "node:test";
import { homeFromInfo, homeStatus, LOCAL_HOME, wantedComputer } from "./home.ts";

const hosts = [{ id: "h1", name: "Praxi host" }];

test("no host -> local; adopted host -> host; offline home -> offline chip; temporary mode -> local data", () => {
  assert.deepEqual(homeFromInfo(null, hosts), LOCAL_HOME);
  assert.equal(wantedComputer(LOCAL_HOME, false, "local"), "local");
  assert.equal(homeStatus(LOCAL_HOME, false, null), "local");

  const home = homeFromInfo("h1", hosts);
  assert.deepEqual(home, { kind: "host", hostId: "h1", name: "Praxi host" });
  assert.equal(wantedComputer(home, false, "local"), "h1");
  assert.equal(homeStatus(home, false, { kind: "connected" }), "online");
  assert.equal(homeStatus(home, false, { kind: "offline" }), "offline");
  assert.equal(homeStatus(home, false, { kind: "connecting" }), "connecting");

  // The explicit temporary mode: this computer's data, with a visible way back.
  assert.equal(wantedComputer(home, true, "local"), "local");
  assert.equal(homeStatus(home, true, { kind: "offline" }), "temporary");
});
