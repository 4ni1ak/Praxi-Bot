import assert from "node:assert/strict";
import { test } from "node:test";
import {
  COMPUTER_FOOTER_KEY,
  computerFooter,
  computerPlaceholderKey,
  isReadyIdle,
  noScreenDetail
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

test("the host's reason is shown as the reason there is no screen", () => {
  assert.equal(noScreenDetail([]), null);
  assert.equal(noScreenDetail([{ kind: "desktop", detail: "  " }]), null);
  assert.equal(
    noScreenDetail([
      { kind: "browser", detail: "other" },
      { kind: "desktop", detail: "Wayland sessions are not supported" }
    ]),
    "Wayland sessions are not supported"
  );
});

test("an available screen nobody watches is ready, not watching live", () => {
  assert.equal(isReadyIdle("watch", false, true), true);
  assert.equal(isReadyIdle("watch", true, true), false);
  assert.equal(isReadyIdle("watch", false, false), false);
  assert.equal(isReadyIdle("active", false, true), false);
});
