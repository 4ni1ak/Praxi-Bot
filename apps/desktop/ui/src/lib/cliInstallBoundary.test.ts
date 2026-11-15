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
  // Installer output (untrusted, may carry raw node traces) is never rendered; only copied.
  assert.doesNotMatch(component, /<pre[^>]*role="log"/);
  assert.match(component, /copyDetails/);
  assert.doesNotMatch(component, /dangerouslySetInnerHTML|\binvoke\(/);
});

test("both locales carry the compact card strings, one status per engine", () => {
  assert.match(component, /status\.\$\{STATUS_KEY\[status\]\}/);
  assert.match(component, /aria-expanded=\{open\}/);
  for (const locale of ["en", "tr"]) {
    const messages = JSON.parse(readFileSync(new URL(`../i18n/locales/${locale}.json`, import.meta.url), "utf8")).settingsModal.ai.detected;
    for (const key of ["ready", "signIn", "outdated", "notInstalled", "unsupported"]) {
      assert.ok(messages.status[key], key);
    }
    assert.ok(messages.detailsToggle && messages.moreEngines && messages.copyDetails);
  }
});
