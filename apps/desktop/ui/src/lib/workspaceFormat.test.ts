import assert from "node:assert/strict";
import { test } from "node:test";
import {
  bytesToGbInput,
  formatBytes,
  gbInputToBytes,
  isStandardWorkspaceSubdir,
  usagePercent,
  workspaceBreadcrumbs
} from "./workspaceFormat.ts";

test("formatBytes formats B, KB, MB, and GB cleanly", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(512), "512 B");
  assert.equal(formatBytes(2048), "2.0 KB");
  assert.equal(formatBytes(5 * 1024 * 1024), "5.00 MB");
  assert.equal(formatBytes(2 * 1024 * 1024 * 1024), "2.00 GB");
});

test("GB limit input conversions round-trip positive values and treat blank as off", () => {
  assert.equal(bytesToGbInput(null), "");
  assert.equal(bytesToGbInput(2 * 1024 ** 3), "2");
  assert.equal(gbInputToBytes(""), null);
  assert.equal(gbInputToBytes("   "), null);
  assert.equal(gbInputToBytes("-1"), undefined);
  assert.equal(gbInputToBytes("abc"), undefined);
  assert.equal(gbInputToBytes("5"), 5 * 1024 ** 3);
});

test("workspaceBreadcrumbs splits normalized relative segments and spots standard folders", () => {
  assert.deepEqual(workspaceBreadcrumbs("leads/2026/q4"), [
    { label: "leads", path: "leads" },
    { label: "2026", path: "leads/2026" },
    { label: "q4", path: "leads/2026/q4" }
  ]);
  assert.equal(isStandardWorkspaceSubdir("notes"), true);
  assert.equal(isStandardWorkspaceSubdir("custom"), false);
  assert.equal(usagePercent(50, 100), 50);
  assert.equal(usagePercent(250, 100), 100);
  assert.equal(usagePercent(10, null), 0);
});
