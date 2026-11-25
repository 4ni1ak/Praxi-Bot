import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const read = (f: string) => readFileSync(fileURLToPath(new URL(f, import.meta.url)), "utf8");

test("unrelated Appearance topics are separate settings groups, not one long card", () => {
  assert.match(read("../views/settings/AppearanceCards.tsx"), /kind: "row",\s*standalone: true/);
  assert.match(read("../views/SettingsModal.tsx"), /p\.card\.kind === "row" && p\.card\.standalone/);
});

test("a settings slot keeps square corners so divider lines stay straight", () => {
  const css = read("../views/SettingsModal.module.css");
  const slot = /\.cardSlot \{\s*border-radius: var\(--space-0\);/;
  assert.match(css, slot);
  assert.doesNotMatch(css, /\.cardSlot,\s*\.blockSlot \{[^}]*border-radius/);
});

test("'service is starting' is a neutral status, not a red alert", () => {
  const tsx = read("../components/ErrorNotice.tsx");
  assert.match(tsx, /h\.code === "serviceStarting"/);
  assert.match(tsx, /role=\{waiting \? "status" : "alert"\}/);
  assert.doesNotMatch(read("../components/EmptyState.module.css"), /dashed/);
});
