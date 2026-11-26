import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const source = readFileSync(new URL("./LiveViewer.tsx", import.meta.url), "utf8");

test("Take control is not offered while there is no picture to control", () => {
  const at = source.indexOf('data-testid="live-view-takeover"');
  assert.ok(at > 0);
  assert.match(source.slice(Math.max(0, at - 400), at), /hasStream \? \(/);
});

test("a timed-out start uses the one 'no screen' explanation, not a second error line", () => {
  assert.match(source, /startTimedOut \? "timeout"/);
  assert.match(source, /!\(stageExplains && startTimedOut\)/);
});

test("a screen that is only starting says so; the 'no screen' explanation is for real failures", () => {
  assert.match(source, /const screenWhy = startingScreen \? null : explainScreen\(/);
  assert.match(source, /startingScreen\s*\?\s*t\("computerCard\.hint\.starting"\)/);
});
