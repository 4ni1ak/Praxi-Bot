import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { mcpErrorHintKey, runtimeFamily } from "./mcpError.ts";

const locale = (name: string) =>
  JSON.parse(
    readFileSync(fileURLToPath(new URL(`../i18n/locales/${name}.json`, import.meta.url)), "utf8")
  );

test("launcher commands map to the installer the user needs", () => {
  assert.equal(runtimeFamily("npx"), "node");
  assert.equal(runtimeFamily(String.raw`C:\Program Files\nodejs\npx.cmd`), "node");
  assert.equal(runtimeFamily("uvx"), "uv");
  assert.equal(runtimeFamily("/usr/local/bin/uvx"), "uv");
  assert.equal(runtimeFamily("pipx"), "pipx");
  assert.equal(runtimeFamily("docker"), "docker");
  assert.equal(runtimeFamily("my-server"), "other");
  assert.equal(runtimeFamily(null), "other");
});

test("a missing runtime gets a command-specific hint, other errors a class hint", () => {
  assert.equal(mcpErrorHintKey("runtime_missing", "uvx"), "apps.error.hint.runtime_uv");
  assert.equal(mcpErrorHintKey("runtime_missing", "npx"), "apps.error.hint.runtime_node");
  assert.equal(mcpErrorHintKey("closed", "npx"), "apps.error.hint.closed");
  assert.equal(mcpErrorHintKey("other", "npx"), null);
  assert.equal(mcpErrorHintKey(undefined, "npx"), null);
});

test("every hint key exists in both locales", () => {
  const kinds = ["runtime_missing", "closed", "spawn", "timeout", "config", "needs_folder"] as const;
  const commands = ["npx", "uvx", "pipx", "docker", "x"];
  for (const lang of ["en", "tr"]) {
    const dict = locale(lang);
    for (const kind of kinds) {
      for (const command of commands) {
        const key = mcpErrorHintKey(kind, command)!;
        const value = key
          .split(".")
          .reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], dict);
        assert.equal(typeof value, "string", `${lang}: ${key}`);
      }
    }
  }
});
