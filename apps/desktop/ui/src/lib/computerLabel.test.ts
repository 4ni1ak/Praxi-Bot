import assert from "node:assert/strict";
import { test } from "node:test";
import {
  COMPUTER_FOOTER_KEY,
  computerFooter,
  computerPlaceholderKey
} from "./computerLabel.ts";

test("no screen yet is only said after a confirmed empty list", () => {
  assert.equal(computerFooter(null, "ok"), "noScreen");
  assert.equal(computerFooter(null, "unknown"), "checking");
  assert.equal(computerFooter(null, "failed"), "queryFailed");
});

test("a known target keeps its own state whatever the query status", () => {
  const up = { available: true, canInput: true };
  assert.equal(computerFooter(up, "failed"), "interactive");
  assert.equal(computerFooter({ available: true, canInput: false }, "ok"), "viewOnly");
  assert.equal(computerFooter({ available: false, canInput: false }, "ok"), "unavailable");
});

test("placeholder text follows the query status", () => {
  assert.equal(computerPlaceholderKey("ok"), "liveView.noTargets");
  assert.equal(computerPlaceholderKey("unknown"), "liveView.checkingTargets");
  assert.equal(computerPlaceholderKey("failed"), "liveView.queryFailed");
  assert.equal(Object.keys(COMPUTER_FOOTER_KEY).length, 6);
});
