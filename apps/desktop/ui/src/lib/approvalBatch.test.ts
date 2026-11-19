import assert from "node:assert/strict";
import { test } from "node:test";
import type { ToolItem } from "../state/timeline.ts";
import { callIdsOf, groupPending, isBatchable, shouldBatch } from "./approvalBatch.ts";

function item(callId: string, name: string, input: unknown, details?: unknown): ToolItem {
  return {
    kind: "tool",
    key: callId,
    callId,
    name,
    input,
    status: "awaiting_approval",
    requestedMs: 0,
    gated: true,
    details
  };
}

test("identical pending calls collapse into one row and are answered together", () => {
  const rows = groupPending([
    item("a", "fs_write", { path: "x.txt", content: "1" }),
    item("b", "fs_write", { content: "1", path: "x.txt" }),
    item("c", "shell_run", { command: "ls" })
  ]);
  assert.equal(rows.length, 2);
  assert.deepEqual(rows[0]?.items.map((i) => i.callId), ["a", "b"]);
  assert.equal(shouldBatch(rows), true);
  assert.deepEqual(callIdsOf(rows, new Set([rows[0]!.key])), ["a", "b"]);
  assert.deepEqual(callIdsOf(rows, new Set()), []);
});

test("a card the user has to read keeps its own card", () => {
  assert.equal(isBatchable(item("a", "web_fetch", {}, { kind: "egress", payload: "x" })), false);
  assert.equal(isBatchable(item("b", "browser_fill", {}, { kind: "secret_fill" })), false);
  assert.equal(isBatchable(item("c", "fs_write", {}, { why: "policy", once_only: false })), true);
  const rows = groupPending([item("d", "fs_write", { path: "a" })]);
  assert.equal(shouldBatch(rows), false);
});
