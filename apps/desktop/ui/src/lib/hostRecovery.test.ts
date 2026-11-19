import assert from "node:assert/strict";
import { test } from "node:test";
import { sendWithHostRecovery } from "./hostRecovery.ts";

const net = { status: 0, code: "network", netKind: "connect", message: "Failed to fetch" };
const timedOut = { status: 0, code: "network", netKind: "timeout", message: "timeout" };
const aborted = { status: 0, code: "network", netKind: "aborted", message: "aborted" };

test("a timeout or an abort never starts the host or re-sends (the daemon may have the message)", async () => {
  for (const err of [timedOut, aborted]) {
    let sends = 0;
    const r = await sendWithHostRecovery({
      send: async () => {
        sends += 1;
        throw err;
      },
      ensure: async () => {
        throw new Error("must not be asked");
      }
    });
    assert.equal(sends, 1);
    assert.equal(r.kind, err === aborted ? "cancelled" : "failed");
  }
});

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
