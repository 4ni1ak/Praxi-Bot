import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const component = readFileSync(new URL("../views/settings/DetectedAccountsBlock.tsx", import.meta.url), "utf8");

test("CLI install UI requires executable descriptor and a single-use expiring approval", () => {
  assert.match(component, /install\?\.executable\s*\?/);
  assert.match(component, /previewCliInstall\(id\)/);
  assert.match(component, /expiresAtMs=\{approval\.expires_at_ms\}/);
  assert.match(component, /approvalRemaining === 0 && !installing/);
  assert.match(component, /onClick=\{\(\) => setApproval\(null\)\}/);
  // Details suppress the shared card's standing-grant controls.
  assert.match(component, /details: \{ command: approval\.command \}/);
  assert.match(component, /<pre[^>]*role="log"[^>]*>\{installOutput\}<\/pre>/);
  assert.doesNotMatch(component, /dangerouslySetInnerHTML|\binvoke\(/);
});

test("both locales explicitly separate bot execution from detection and installation", () => {
  assert.match(component, /engine\?\.limitation === "detection_only"/);
  assert.match(component, /engine\?\.limitation === "codex_unconfinable"/);
  assert.match(component, /row\.runnable \?/);
  for (const locale of ["en", "tr"]) {
    const messages = JSON.parse(readFileSync(new URL(`../i18n/locales/${locale}.json`, import.meta.url), "utf8")).settingsModal.ai.detected;
    assert.ok(messages.runnableEngine);
    assert.match(messages.description, /Claude Code/);
    assert.match(messages.description, /Codex/);
    assert.ok(messages.detectionOnly);
    assert.match(messages.codexBlocked, /Codex/);
    assert.match(messages.nativeToolsOpen, /OpenCode/);
    assert.ok(messages.nativeToolsOpen.length > 60);
  }
});
