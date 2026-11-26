import assert from "node:assert/strict";
import { test } from "node:test";
import { siteRuleTarget } from "./siteRules.ts";

test("a program is shown by file name in lists and by full path when confirming", () => {
  const win = String.raw`C:\Users\a\AppData\Local\Temp\notepad.exe`;
  assert.equal(siteRuleTarget("app", win), "notepad.exe");
  assert.equal(siteRuleTarget("app", win, true), win);
  assert.equal(siteRuleTarget("app", "/usr/bin/firefox"), "firefox");
  assert.equal(siteRuleTarget("app", "/usr/bin/firefox", true), "/usr/bin/firefox");
});

test("a site is always shown as its host", () => {
  assert.equal(siteRuleTarget("site", "example.com"), "example.com");
  assert.equal(siteRuleTarget("site", "example.com", true), "example.com");
});
