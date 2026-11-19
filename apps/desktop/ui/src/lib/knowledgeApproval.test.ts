import assert from "node:assert/strict";
import { test } from "node:test";
import { brainDetails } from "./approvalDetails.ts";

test("brain approval preserves the exact frozen bytes and hash", () => {
  const content = "---\ntrust: untrusted\n---\n<script>do not execute</script>";
  const details = brainDetails({ kind: "knowledge_brain_write", path: "brain/decision.md", content, sha256: "a".repeat(64), sources: [{ id: "source-1", revision: 3, label: "Evidence" }] });
  assert.equal(details?.content, content);
  assert.equal(details?.sources[0].revision, 3);
  assert.equal(brainDetails({ kind: "knowledge_brain_write", content }), null);
});

test("malformed brain cards cannot hide unbounded content or source metadata", () => {
  const base = { kind: "knowledge_brain_write", path: "brain/x.md", content: "x", sha256: "a".repeat(64), sources: [] };
  assert.equal(brainDetails({ ...base, content: "x".repeat(65_537) }), null);
  assert.equal(brainDetails({ ...base, sources: [{ id: "x", revision: "3", label: "source" }] }), null);
});
