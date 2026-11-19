import assert from "node:assert/strict";
import { test } from "node:test";
import { sendWithHostRecovery } from "./hostRecovery.ts";

const net = { status: 0, code: "network", message: "Failed to fetch" };

test("a dead local service is started, then the message is sent once more", async () => {
  const sent: unknown[] = [];
  let starting = 0;
  const r = await sendWithHostRecovery({
    send: async (c) => {
      sent.push(c);
      if (c === null) {
        throw net;
      }
      return "ok";
    },
    ensure: async () => ({ kind: "started", client: "fresh" }),
    onStarting: () => {
      starting += 1;
    }
  });
  assert.deepEqual(r, { kind: "sent", value: "ok", started: true });
  assert.deepEqual(sent, [null, "fresh"]);
  assert.equal(starting, 1);
});

test("a remote host is never started and the draft is kept; a live service is not re-sent", async () => {
  let sends = 0;
  const send = async () => {
    sends += 1;
    throw net;
  };
  const remote = await sendWithHostRecovery({ send, ensure: async () => ({ kind: "remote" }) });
  assert.equal(remote.kind, "remote_offline");
  const live = await sendWithHostRecovery({ send, ensure: async () => ({ kind: "reachable" }) });
  assert.equal(live.kind, "failed");
  assert.equal(sends, 2, "one attempt each, no duplicate send");
  const other = await sendWithHostRecovery({
    send: async () => {
      throw { status: 403, code: "forbidden", message: "no" };
    },
    ensure: async () => {
      throw new Error("must not start the host for a refusal");
    }
  });
  assert.equal(other.kind, "failed");
});

test("a start that fails is reported as such", async () => {
  const r = await sendWithHostRecovery({
    send: async () => {
      throw net;
    },
    ensure: async () => {
      throw new Error("port in use");
    }
  });
  assert.equal(r.kind, "start_failed");
});
