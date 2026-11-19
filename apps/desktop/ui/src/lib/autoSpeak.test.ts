import assert from "node:assert/strict";
import { test } from "node:test";
import { repliesToSpeak } from "./autoSpeak.ts";

const run = (id: string, over: Record<string, unknown> = {}) => ({
  id,
  finishedMs: 5,
  error: null,
  stopped: false,
  items: [{ kind: "text" as const, key: "t", ts: 1, text: `reply ${id}` }],
  ...over
});

test("history is primed silently, then each finished reply is spoken once", () => {
  const seen = new Set<string>();
  assert.deepEqual(repliesToSpeak([run("a")], seen, true), []);
  assert.deepEqual(repliesToSpeak([run("a"), run("b", { finishedMs: undefined })], seen, false), []);
  const done = [run("a"), run("b"), run("c", { error: "boom" })];
  assert.deepEqual(repliesToSpeak(done, seen, false), [{ id: "b", text: "reply b" }]);
  assert.deepEqual(repliesToSpeak(done, seen, false), []);
});
