import assert from "node:assert/strict";
import { test } from "node:test";
import { canAnswer, connectionState } from "./connectivity.ts";
import { canAutoDeliver, enqueue, EMPTY_QUEUE, MAX_QUEUED_MESSAGES, queueHasRoom, removeQueued } from "./messageQueue.ts";

test("fake host observations distinguish offline from an available configured local fallback", () => {
  const report = { services: [
    { id: "cloud", local: false, reachable: false },
    { id: "installed", local: true, reachable: true }
  ] };
  assert.equal(connectionState(report), "offline");
  assert.equal(canAnswer(report, ["cloud/model"]), false);
  assert.equal(canAnswer(report, ["cloud/model", "installed/model"]), true);
  assert.equal(canAnswer(report, ["cli:codex/auto"]), true, "CLI availability remains unknown");
  assert.equal(connectionState(null), "unknown");
  assert.equal(connectionState({ services: [] }), "unknown");
  assert.equal(connectionState({ services: [{ id: "cloud", local: false, reachable: true }] }), "online");
});

test("a later Stop holds offline messages until a deliberate send, including after reopening the chat", () => {
  const queue = enqueue(EMPTY_QUEUE, "wait for connection", "a", 1, true, 7);
  assert.equal(canAutoDeliver(queue[0], 7), true);
  assert.equal(canAutoDeliver(queue[0], 8), false);
});

test("offline drafts are bounded, marked, cancellable, and a full queue keeps the draft", () => {
  let queue = EMPTY_QUEUE;
  for (let i = 0; i < MAX_QUEUED_MESSAGES; i++) queue = enqueue(queue, "draft", String(i), i, true);
  assert.equal(queue.every((m) => m.offline), true);
  assert.equal(queueHasRoom(queue, "keep in composer"), false);
  assert.equal(enqueue(queue, "keep in composer", "extra", 30, true), queue);
  queue = removeQueued(queue, "0");
  assert.equal(queueHasRoom(queue, "replacement"), true);
  assert.equal(enqueue(EMPTY_QUEUE, "x".repeat(32_001), "large", 0, true), EMPTY_QUEUE);
});
