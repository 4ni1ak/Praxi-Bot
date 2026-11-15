import assert from "node:assert/strict";
import { test } from "node:test";
import { MockApiClient } from "../api/mock.ts";
import {
  cliStatus,
  displayPath,
  hasSwitch,
  needsProviderReload,
  shortVersion,
  splitEngines
} from "./cliCards.ts";

test("one status per engine, one card for codex, installed engines never offer install", async () => {
  const engines = await new MockApiClient().cliEngines();
  const status = Object.fromEntries(engines.map((e) => [e.detected.id, cliStatus(e)]));
  assert.deepEqual(status, {
    claude: "ready",
    codex: "ready",
    cursor: "not_installed",
    opencode: "ready",
    gemini: "not_installed",
    agy: "unsupported"
  });
  assert.equal(engines.filter((e) => e.detected.id.includes("chatgpt")).length, 0);
  const { installed, available } = splitEngines(engines);
  assert.ok(installed.every((e) => e.detected.installed));
  assert.deepEqual(available.map((e) => e.detected.id), ["cursor", "gemini"]);
  assert.equal(hasSwitch(engines.find((e) => e.detected.id === "agy")!), false);
});

test("paths lose the verbatim prefix and versions read as numbers", () => {
  assert.equal(displayPath("\\\\?\\C:\\Users\\emre\\npm\\claude.cmd"), "C:\\Users\\emre\\npm\\claude.cmd");
  assert.equal(displayPath("\\\\?\\UNC\\srv\\share\\x"), "\\\\srv\\share\\x");
  assert.equal(shortVersion("2.1.325 (Claude Code)"), "2.1.325");
  assert.equal(shortVersion("opencode v2.3.0"), "2.3.0");
  assert.equal(shortVersion(null), null);
});

test("an engine the daemon switched on by itself triggers one provider reload", async () => {
  const engines = await new MockApiClient().cliEngines();
  const on = engines.map((e) => (e.detected.id === "claude" ? { ...e, enabled: true } : e));
  assert.equal(needsProviderReload(on, new Set()), true);
  assert.equal(needsProviderReload(on, new Set(["cli:claude"])), false);
});
