import assert from "node:assert/strict";
import { test } from "node:test";
import {
  computerScopePlan,
  deriveHostState,
  hostAcceptsWork,
  liveRunIds,
  type HostHealthLike,
  type HostLinkLike
} from "./hostState.ts";

const link = (state: HostLinkLike["state"]): HostLinkLike => ({
  id: "h1",
  name: "Oracle",
  state
});
const health = (over: Partial<HostHealthLike> = {}): HostHealthLike => ({
  status: "ok",
  version: "0.1.0",
  api_version: "1",
  ...over
});
const base = { localVersion: "0.1.0", localApiVersion: "1" };

test("a host that is not in the paired list, or revoked us, is not paired", () => {
  assert.equal(
    deriveHostState({ link: null, health: null, ...base }).kind,
    "not_paired"
  );
  assert.equal(
    deriveHostState({ link: link("unreachable"), health: null, ...base }).kind,
    "not_paired"
  );
});

test("offline, and connecting until the host answered health", () => {
  assert.equal(
    deriveHostState({ link: link("offline"), health: null, ...base }).kind,
    "offline"
  );
  for (const state of ["idle", "connecting"] as const) {
    assert.equal(
      deriveHostState({ link: link(state), health: null, ...base }).kind,
      "connecting"
    );
  }
  // Channel up but the health probe has not come back yet.
  assert.equal(
    deriveHostState({ link: link("connected"), health: null, ...base }).kind,
    "connecting"
  );
});

test("connected and compatible", () => {
  const s = deriveHostState({ link: link("connected"), health: health(), ...base });
  assert.equal(s.kind, "connected");
  assert.equal(hostAcceptsWork(s), true);
});

test("a different API or minor version blocks and shows both versions", () => {
  for (const h of [health({ api_version: "2" }), health({ version: "0.2.0" })]) {
    const s = deriveHostState({ link: link("connected"), health: h, ...base });
    assert.equal(s.kind, "version_mismatch");
    if (s.kind === "version_mismatch") {
      assert.equal(s.local, "0.1.0");
      assert.equal(s.host, h.version);
      assert.equal(s.blocking, true);
    }
    assert.equal(hostAcceptsWork(s), false);
  }
});

test("a patch-only difference warns but does not block", () => {
  const s = deriveHostState({
    link: link("connected"),
    health: health({ version: "0.1.3" }),
    ...base
  });
  assert.equal(s.kind, "version_mismatch");
  assert.equal(hostAcceptsWork(s), true);
});

test("setup_required on a compatible host is the no-model-key state", () => {
  const s = deriveHostState({
    link: link("connected"),
    health: health({ status: "setup_required" }),
    ...base
  });
  assert.equal(s.kind, "no_model_key");
  assert.equal(hostAcceptsWork(s), false);
});

test("a version mismatch wins over a missing key", () => {
  const s = deriveHostState({
    link: link("connected"),
    health: health({ status: "setup_required", api_version: "9" }),
    ...base
  });
  assert.equal(s.kind, "version_mismatch");
});

test("only live runs are listed for stop", () => {
  assert.deepEqual(
    liveRunIds([
      { id: "a", state: "running" },
      { id: "b", state: "completed" },
      { id: "c", state: "waiting_approval" },
      { id: "d", state: "paused" },
      { id: "e", state: "queued" }
    ]),
    ["a", "c", "e"]
  );
});

test("the local control bridge stays mounted while a host is selected", () => {
  assert.deepEqual(computerScopePlan("local"), { localBridge: true, hostSession: false });
  assert.deepEqual(computerScopePlan("host-key"), { localBridge: true, hostSession: true });
});
