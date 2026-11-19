import assert from "node:assert/strict";
import { test } from "node:test";
import type { SessionInfo, TargetInfo } from "../api/types.ts";
import { runReconnectLoop, type SessionLink } from "./liveViewReconnect.ts";
import {
  applyControlStateDto,
  applyFrameRendered,
  applySessionCreated,
  applyStatusRecord,
  applyStreamDisconnected,
  applyTakeoverAccepted,
  buildTakeoverRequest,
  canSendLiveInput,
  createInitialLiveViewState,
  describeRecoveryPrompt,
  type LiveViewControllerState
} from "./liveViewState.ts";
import type { FrameHeader, StatusHeader } from "./liveViewWire.ts";

const target: TargetInfo = {
  target_id: "t-1",
  bot_id: "bot-1",
  run_id: "run-1",
  host_id: "local",
  kind: "browser",
  generation: 2,
  capabilities: { codecs: ["jpeg"], view: true, input: true, reason: null },
  unavailable_reason: null
};

const mkSession = (id: string): SessionInfo => ({
  session_id: id,
  generation: 2,
  expires_in_ms: 20_000,
  input_capabilities: true,
  effective_profile: {
    profile: "watch",
    codec: "jpeg",
    max_width: 960,
    max_height: 600,
    max_fps: 1,
    quality: 60,
    burst_bytes: 2_000_000,
    bytes_per_second: 1_000_000
  },
  stream_url: `/v1/view/sessions/${id}/frames`
});

const frame = (sid: string): FrameHeader => ({
  v: 1,
  type: "frame",
  session_id: sid,
  sent_at_ms: 1,
  target_id: "t-1",
  generation: 2,
  frame_seq: 1,
  captured_at_ms: 1,
  cause: "refresh",
  action_seq: null,
  codec: "jpeg",
  width: 1280,
  height: 800,
  source_rect: { x: 0, y: 0, width: 1280, height: 800, units: "physical_px" },
  geometry_revision: 7,
  repeated: false
});

const status = (sid: string, state: "active" | "watch"): StatusHeader => ({
  v: 1,
  type: "status",
  session_id: sid,
  state,
  reason: null,
  effective_profile: mkSession(sid).effective_profile,
  control_epoch: state === "watch" ? 0 : 5,
  input_allowed: state === "active"
});

/** Fake clock + sleep: sleeping advances time; every loop turn is instant. */
function clock() {
  let t = 1_000_000;
  return {
    now: () => t,
    advance: (ms: number) => void (t += ms),
    sleep: async (ms: number) => void (t += ms)
  };
}

const noHooks = {
  onSession: () => undefined,
  onDisconnected: () => undefined,
  beforeRetry: () => undefined
};

test("backoff grows while a stream accepts and drops, and only resets after a healthy stretch", async () => {
  const c = clock();
  const ctl = new AbortController();
  const delays: number[] = [];
  let opens = 0;
  const link: SessionLink = {
    createSession: async () => mkSession("s-1"),
    openStream: async () => {
      opens += 1;
      // accepts, writes a status, drops 50 ms later (a restart loop)
      c.advance(opens === 7 ? 11_000 : 50);
      if (opens === 8) {
        ctl.abort();
      }
    },
    deleteSession: async () => undefined
  };
  await runReconnectLoop(link, noHooks, {
    signal: ctl.signal,
    sleep: c.sleep,
    now: c.now,
    onBackoff: (ms) => delays.push(ms)
  });
  assert.deepEqual(delays, [500, 1000, 2000, 4000, 4000, 4000, 500]);
});

test("a dropped stream is reopened on the same session; a vanished session is recreated", async () => {
  const c = clock();
  const ctl = new AbortController();
  const created: string[] = [];
  const opened: string[] = [];
  const deleted: string[] = [];
  const link: SessionLink = {
    createSession: async () => {
      const id = `s-${created.length + 1}`;
      created.push(id);
      return mkSession(id);
    },
    openStream: async (id) => {
      opened.push(id);
      if (opened.length === 2) {
        throw Object.assign(new Error("gone"), { status: 404 });
      }
      if (opened.length === 3) {
        ctl.abort();
      }
    },
    deleteSession: async (id) => void deleted.push(id)
  };
  await runReconnectLoop(link, noHooks, {
    signal: ctl.signal,
    sleep: c.sleep,
    now: c.now
  });
  assert.deepEqual(opened, ["s-1", "s-1", "s-2"]);
  assert.deepEqual(created, ["s-1", "s-2"]);
  assert.deepEqual(deleted, [], "no delete on a transient drop");
});

/** Run the loop wired to the same reducers LiveViewContext uses. */
async function reconnectScenario(opts: { recreateSession: boolean }) {
  const c = clock();
  const ctl = new AbortController();
  let st: LiveViewControllerState = createInitialLiveViewState(target);
  const update = (f: (p: LiveViewControllerState) => LiveViewControllerState) => {
    st = f(st);
  };
  let created = 0;
  let opens = 0;
  const link: SessionLink = {
    createSession: async () => mkSession(`s-${++created}`),
    openStream: async (sid) => {
      opens += 1;
      if (opens === 1) {
        // first connect: we take control, then the stream drops
        update((p) => applyFrameRendered(p, frame(sid), 1000));
        update((p) =>
          applyTakeoverAccepted(p, {
            takeover_id: "to-1",
            control_epoch: 5,
            state: "active"
          })
        );
        update((p) => applyStatusRecord(p, status(sid, "active"), 1010));
        assert.equal(canSendLiveInput(st, 1020).ok, true);
        return;
      }
      if (opens === 2 && opts.recreateSession) {
        throw Object.assign(new Error("gone"), { status: 404 });
      }
      // the daemon reports watch for a session that does not own the paused takeover
      update((p) => applyStatusRecord(p, status(sid, "watch"), 2000));
      ctl.abort();
    },
    deleteSession: async () => undefined
  };
  await runReconnectLoop(
    link,
    {
      onSession: (s) => update((p) => applySessionCreated(p, target, s)),
      onDisconnected: () => update((p) => applyStreamDisconnected(p)),
      beforeRetry: () => undefined
    },
    { signal: ctl.signal, sleep: c.sleep, now: c.now }
  );
  return st;
}

test("after a reconnect while holding control the dock is paused + reacquire, never watch + take control", async () => {
  for (const recreateSession of [false, true]) {
    const st = await reconnectScenario({ recreateSession });
    assert.equal(st.viewState, "paused", `recreate=${recreateSession}`);
    assert.equal(st.reason, "disconnected");
    assert.equal(canSendLiveInput(st, 5000).ok, false);
    const prompt = describeRecoveryPrompt(st, null);
    assert.equal(prompt?.mode, "reacquire");
    assert.equal(prompt?.takeoverId, "to-1");
    const req = buildTakeoverRequest(st, "req-r", {
      reacquireTakeoverId: prompt?.takeoverId
    });
    assert.equal(req?.reacquire_takeover_id, "to-1");

    // a fresh control snapshot (paused / disconnected, our id) keeps it that way
    const healed = applyControlStateDto(st, {
      app_connected: true,
      expires_in_ms: 15_000,
      latched: "paused",
      mode: "paused",
      release: null,
      takeover: {
        id: "to-1",
        control_epoch: 6,
        input_expires_in_ms: 0,
        owner: "local-ui:1",
        reason: "disconnected",
        session_id: "s-1",
        state: "paused",
        target_id: "t-1"
      }
    });
    assert.equal(healed.viewState, "paused");
    assert.equal(describeRecoveryPrompt(healed, null)?.mode, "reacquire");
  }
});
