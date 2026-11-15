import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("./RunChangesSection.tsx", import.meta.url), "utf8");

test("changes fetch only on open, paginate, refresh conflicts and expose safety undo", () => {
  assert.match(source, /if \(open\) void loadChanges\(\)/);
  assert.match(source, /getRunChanges\(runId, offset\)/);
  assert.match(source, /changes: \[\.\.\.previous.changes, \.\.\.next.changes\]/);
  assert.match(source, /e.code === "modified_since" \|\| e.code === "could_not_verify"/);
  const conflict = source.slice(source.indexOf('if (e instanceof ApiError'));
  assert.ok(conflict.indexOf("setPreview(null)") < conflict.indexOf("await client.previewRestore"));
  assert.ok(conflict.indexOf("setOverwriteChecked(false)") < conflict.indexOf("await client.previewRestore"));
  assert.match(source, /disabled=\{!gate.canConfirm\}/);
  assert.match(source, /setSafetyIds\(res.safety_checkpoint_ids\)/);
  assert.match(source, /openRestoreModal\(id, null\)/);
});

test("unknown verification uses distinct status, warning and consent in the real component", () => {
  assert.match(source, /verificationLabel\(change\)/);
  assert.match(source, /verificationLabel\(entry\)/);
  assert.match(source, /restoreConsentKeys\(preview\).warning/);
  assert.match(source, /restoreConsentKeys\(preview\).label/);
});
