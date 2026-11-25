import assert from "node:assert/strict";
import { test } from "node:test";
import {
  checkOf,
  copyName,
  editSteps,
  importProblem,
  MAX_IMPORT_BYTES,
  moveRow,
  removeRow,
  rowsOf,
  stepParams,
  stepView,
  waitProblem
} from "./workflowEdit.ts";

test("a typed secret is described by its parameter name only", () => {
  const v = stepView({ kind: "browser_action", action: "type", target: { role: "textbox", name: "Password" }, secret: "pw" });
  assert.deepEqual([v.action, v.target, v.value, v.secretName], ["type", "textbox Password", null, "pw"]);
  assert.equal(stepView({ kind: "navigate", url: "https://x.test/{who}" }).target, "https://x.test/{who}");
  assert.deepEqual(stepParams({ kind: "navigate", url: "https://x.test/{who}/{who}/{id}" }), ["who", "id"]);
});

test("reordering and deleting keep the original step numbers", () => {
  let rows = moveRow(rowsOf(3), 2, -1);
  rows = removeRow(rows, 0);
  assert.deepEqual(editSteps(rows), [{ keep: 2 }, { keep: 1 }]);
  assert.equal(moveRow(rows, 0, -1), rows);
});

test("wait steps need a condition and a sane timeout", () => {
  assert.equal(waitProblem({ mode: "text", value: " ", timeoutSecs: 5 }), "empty");
  assert.equal(waitProblem({ mode: "idle", value: "", timeoutSecs: 9999 }), "timeout");
  assert.equal(waitProblem({ mode: "idle", value: "", timeoutSecs: 5 }), null);
  assert.deepEqual(editSteps([{ wait: { mode: "window", value: "Inbox", timeoutSecs: 7 } }]), [
    { wait: { text: null, window_title: "Inbox", idle: false, timeout_secs: 7 } }
  ]);
});

test("import pre-check refuses big, non-JSON and non-workflow text", () => {
  assert.equal(importProblem("x".repeat(MAX_IMPORT_BYTES + 1)), "tooBig");
  assert.equal(importProblem("nope"), "notJson");
  assert.equal(importProblem("[]"), "notWorkflow");
  assert.equal(importProblem('{"steps":[]}'), null);
});

test("final check round-trips and copy names stay unique", () => {
  assert.deepEqual(checkOf({ kind: "window_title", pattern: "Done" }), { kind: "window_title", text: "Done" });
  assert.equal(checkOf(null), null);
  assert.equal(copyName("a", ["a-copy", "a-copy-2"]), "a-copy-3");
});
