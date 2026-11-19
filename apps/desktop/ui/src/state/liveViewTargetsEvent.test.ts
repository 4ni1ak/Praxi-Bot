import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("./LiveViewContext.tsx", import.meta.url), "utf8");

test("an open panel refetches its targets on view_targets_changed, without control events (244-V #3)", () => {
  assert.match(source, /env\.event\.type === "view_targets_changed"/);
  const handler = source.slice(source.indexOf("view_targets_changed"));
  assert.ok(handler.indexOf("refreshTargets()") > 0);
  // Bot-scoped sessions carry the focused bot so the daemon can refuse another bot's browser.
  assert.match(source, /bot_id: botFocus/);
});
