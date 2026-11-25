import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { EXIT_MS, exitDelayMs } from "./presenceTiming.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");

test("a leaving surface waits one exit, and not at all when motion is reduced", () => {
  assert.equal(exitDelayMs(false), EXIT_MS);
  assert.equal(exitDelayMs(true), 0);
  assert.equal(exitDelayMs(false, 300), 300);
  // A little over the 120 ms exit token, so the last frame is never cut.
  assert.ok(EXIT_MS > 120 && EXIT_MS <= 200);
});

test("Modal, Popover, ContextMenu and Toast keep leaving surfaces mounted for their exit and make them inert", () => {
  for (const file of ["Modal", "Popover", "ContextMenu", "Toast"]) {
    const tsx = read(`../components/${file}.tsx`);
    const css = read(`../components/${file}.module.css`);
    assert.match(tsx, /usePresence|motionIsReduced/, `${file} uses the presence helper`);
    assert.match(tsx, /data-state/, `${file} marks its closing state`);
    assert.match(css, /\[data-state="closing"\]/, `${file} has an exit style`);
    assert.match(css, /--motion-duration-exit/, `${file} exits with the exit token`);
  }
});
