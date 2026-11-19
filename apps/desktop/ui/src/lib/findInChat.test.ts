import assert from "node:assert/strict";
import { test } from "node:test";
import { findMatches, stepMatch } from "./findInChat.ts";

test("finds every case-insensitive occurrence per text node", () => {
  const m = findMatches(["Hello hello", "nothing", "HELLO"], "hello");
  assert.deepEqual(
    m.map((x) => [x.node, x.start, x.end]),
    [
      [0, 0, 5],
      [0, 6, 11],
      [2, 0, 5]
    ]
  );
});

test("an empty or blank query matches nothing", () => {
  assert.deepEqual(findMatches(["abc"], ""), []);
  assert.deepEqual(findMatches(["abc"], "   "), []);
});

test("stepping wraps in both directions and handles no matches", () => {
  assert.equal(stepMatch(2, 3, 1), 0);
  assert.equal(stepMatch(0, 3, -1), 2);
  assert.equal(stepMatch(-1, 3, 1), 0);
  assert.equal(stepMatch(-1, 3, -1), 2);
  assert.equal(stepMatch(0, 0, 1), -1);
});
