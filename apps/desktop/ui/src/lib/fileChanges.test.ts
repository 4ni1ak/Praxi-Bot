import assert from "node:assert/strict";
import { test } from "node:test";
import type { RestorePreview } from "../api/types.ts";
import {
  displayPath,
  evaluateRestoreGate,
  languageForPath,
  parseUnifiedDiff
} from "./fileChanges.ts";

test("parseUnifiedDiff tracks hunk headers, line numbers, and keeps text inert", () => {
  const patch = [
    "--- /work/app.ts",
    "+++ /work/app.ts",
    "@@ -10,3 +10,3 @@",
    " const x = 1;",
    "-const y = '<img src=x onerror=alert(1)>';",
    "+const y = 'safe';",
    " export { x, y };",
    ""
  ].join("\n");

  const rows = parseUnifiedDiff(patch);
  assert.deepEqual(rows, [
    { kind: "hunk", oldLine: null, newLine: null, text: "@@ -10,3 +10,3 @@" },
    { kind: "context", oldLine: 10, newLine: 10, text: "const x = 1;" },
    {
      kind: "del",
      oldLine: 11,
      newLine: null,
      text: "const y = '<img src=x onerror=alert(1)>';"
    },
    { kind: "add", oldLine: null, newLine: 11, text: "const y = 'safe';" },
    { kind: "context", oldLine: 12, newLine: 12, text: "export { x, y };" }
  ]);
});

test("evaluateRestoreGate blocks confirm when modified_since is non-empty until checked", () => {
  const cleanPreview: RestorePreview = {
    run_id: "r1",
    checkpoint_id: "cp-1",
    entries: [
      { path: "/tmp/a.txt", accessible: true, changed_by_run: null, via_link_of: null, current_matches_after: true, action: "restore" }
    ],
    modified_since: [],
    covered: ["fs_write"],
    not_covered: ["shell"]
  };

  assert.deepEqual(evaluateRestoreGate(null, false, false), {
    needsOverwriteConfirm: false,
    canConfirm: false,
    requiresForce: false
  });

  assert.deepEqual(evaluateRestoreGate(cleanPreview, false, false), {
    needsOverwriteConfirm: false,
    canConfirm: true,
    requiresForce: false
  });

  assert.deepEqual(evaluateRestoreGate(cleanPreview, false, true), {
    needsOverwriteConfirm: false,
    canConfirm: false,
    requiresForce: false
  });

  assert.equal(evaluateRestoreGate(cleanPreview, false, false, true).canConfirm, false);
  assert.equal(evaluateRestoreGate(cleanPreview, true, false, true).requiresForce, true);
  assert.equal(evaluateRestoreGate({ ...cleanPreview, entries: [{ ...cleanPreview.entries[0], accessible: false, action: "keep" }] }, true, false).canConfirm, false);

  const dirtyPreview: RestorePreview = {
    ...cleanPreview,
    entries: [
      { path: "/tmp/a.txt", accessible: true, changed_by_run: null, via_link_of: null, current_matches_after: false, action: "restore" }
    ],
    modified_since: ["/tmp/a.txt"]
  };

  assert.deepEqual(evaluateRestoreGate(dirtyPreview, false, false), {
    needsOverwriteConfirm: true,
    canConfirm: false,
    requiresForce: false
  });

  assert.deepEqual(evaluateRestoreGate(dirtyPreview, true, false), {
    needsOverwriteConfirm: true,
    canConfirm: true,
    requiresForce: true
  });
});

test("languageForPath and displayPath normalize extensions and verbatim prefixes", () => {
  assert.equal(languageForPath("/repo/src/main.rs"), "rust");
  assert.equal(languageForPath("C:\\work\\index.tsx"), "tsx");
  assert.equal(languageForPath("Makefile"), "");
  assert.equal(displayPath("\\\\?\\C:\\work\\a.txt"), "C:\\work\\a.txt");
  assert.equal(displayPath("\\\\?\\UNC\\srv\\share\\a.txt"), "\\\\srv\\share\\a.txt");
  assert.equal(displayPath("/tmp/a.txt"), "/tmp/a.txt");
});
