import assert from "node:assert/strict";
import { test } from "node:test";
import { EMPTY_QUEUE, enqueue, takeForRunEnd, type MessageQueue, type QueuedMessage } from "./messageQueue.ts";
import { createQueueDelivery } from "./queueDelivery.ts";

/** The synchronous per-chat store behind useChatQueue, plus the two delivery effects of ChatView. */
function chat(opts: { failSends?: number } = {}) {
  let store: MessageQueue = EMPTY_QUEUE;
  const update = (f: (q: MessageQueue) => MessageQueue) => {
    store = f(store);
  };
  const delivery = createQueueDelivery(update);
  const sent: string[] = [];
  let sending = false; // queueSending.current
  let failures = opts.failSends ?? 0;
  const deliverQueued = (message: QueuedMessage) =>
    delivery.deliver(message, !sending, async (m, hold) => {
      sending = true;
      try {
        await Promise.resolve();
        if (failures > 0) {
          failures -= 1;
          throw new Error("send failed");
        }
        sent.push(m.text);
      } catch {
        hold({ offline: false, stopEpoch: -1 });
      } finally {
        sending = false;
      }
    });
  /** Both effects run in the same commit on the same (stale) render snapshot. */
  const commit = (was: boolean, active: boolean) => {
    const snapshot = store;
    const { message } = takeForRunEnd(snapshot, was, active);
    if (message) deliverQueued(message); // effect 1: run end
    const first = snapshot[0]; // effect 2: offline queue delivery, connection is back
    if (!active && first?.offline) deliverQueued(first);
  };
  return {
    queue: () => store,
    sent,
    update,
    commit,
    flush: () => new Promise((r) => setTimeout(r, 0))
  };
}

test("an offline-queued message is delivered once when run end and reconnect land in one commit", async () => {
  const c = chat();
  c.update((q) => enqueue(q, "send the report", "a", 1, true));
  c.commit(true, false);
  await c.flush();
  assert.deepEqual(c.sent, ["send the report"]);
  assert.equal(c.queue().length, 0, "the delivered message must not be back in the queue");
  c.commit(true, false); // the next run end
  await c.flush();
  assert.deepEqual(c.sent, ["send the report"], "no second delivery at the next run end");
});

test("a failed delivery goes back into the queue exactly once", async () => {
  const c = chat({ failSends: 1 });
  c.update((q) => enqueue(q, "retry me", "a", 1, true));
  c.commit(true, false);
  await c.flush();
  assert.deepEqual(c.sent, []);
  assert.deepEqual(c.queue().map((m) => m.id), ["a"], "back once, not twice");
  assert.equal(c.queue()[0].offline, false);
  assert.equal(c.queue()[0].stopEpoch, -1, "an uncertain send is never replayed automatically");
});

test("two queued messages are delivered in order, once each", async () => {
  const c = chat();
  c.update((q) => enqueue(enqueue(q, "first", "a", 1, true), "second", "b", 2, true));
  c.commit(true, false);
  await c.flush();
  assert.deepEqual(c.sent, ["first"]);
  assert.deepEqual(c.queue().map((m) => m.id), ["b"]);
  c.commit(true, false);
  await c.flush();
  assert.deepEqual(c.sent, ["first", "second"]);
  assert.equal(c.queue().length, 0);
  c.commit(true, false);
  await c.flush();
  assert.deepEqual(c.sent, ["first", "second"]);
});
