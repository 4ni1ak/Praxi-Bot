import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { startupTone } from "./startupTone.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");

test("a service that is still starting is neutral; only a failure is a problem", () => {
  for (const stage of ["idle", "spawning", "waiting", "ready"] as const) {
    assert.equal(startupTone(stage), "progress", stage);
  }
  for (const stage of ["sidecar_missing", "sidecar_blocked", "spawn_failed", "exited", "not_responding", "port_in_use", "unreachable"] as const) {
    assert.equal(startupTone(stage), "problem", stage);
  }
});

test("the start-up screen paints the danger colour only for a problem, and the offline screen shows 'starting' neutral", () => {
  const css = read("../components/StartupProblem.module.css");
  assert.match(css, /\.root\[data-tone="problem"\] \.icon\s*\{[^}]*--color-danger/);
  assert.doesNotMatch(css.replace(/\.root\[data-tone="problem"\] \.icon\s*\{[^}]*\}/, ""), /color:\s*var\(--color-danger\)/);
  const app = read("../App.tsx");
  assert.match(app, /serviceProblem === "starting"/);
  assert.match(app, /<StartingNotice/);
});
