import assert from "node:assert/strict";
import { test } from "node:test";
import { deriveView, formatLive, IDLE_PAUSE_MS, measureFps, type ViewInputs } from "./computerView.ts";
import { gradientBase, gradientVars, hexToOklch, NEUTRAL_CHROMA } from "./botGradient.ts";

const base: ViewInputs = {
  wanted: false,
  pausedIdle: false,
  issue: null,
  targetsStatus: "unknown",
  hasTarget: false,
  targetUnavailable: false,
  noScreen: null,
  permissionRefused: false,
  hasFrame: false
};

test("the computer card never starts a view by itself: idle until asked, then starting, live, paused", () => {
  assert.equal(deriveView(base).kind, "idle");
  // a panel that merely opens (targets known, a frame exists elsewhere) is still idle
  assert.equal(deriveView({ ...base, targetsStatus: "ok", hasTarget: true, hasFrame: true }).kind, "idle");
  assert.equal(deriveView({ ...base, wanted: true }).kind, "starting");
  assert.equal(deriveView({ ...base, wanted: true, targetsStatus: "ok", hasTarget: true, hasFrame: true }).kind, "live");
  // the quiet-time timer ended the view
  const paused = deriveView({ ...base, pausedIdle: true });
  assert.equal(paused.kind, "paused");
  assert.equal(paused.paused, "idle");
  assert.equal(IDLE_PAUSE_MS, 180_000);
});

test("an unreachable server or a refused screen is 'unavailable' with a reason, not a spinner", () => {
  assert.deepEqual(deriveView({ ...base, issue: "offline" }), { kind: "unavailable", unavailable: "offline", paused: null });
  assert.equal(deriveView({ ...base, issue: "connecting" }).kind, "idle");
  assert.equal(deriveView({ ...base, issue: "connecting", wanted: true }).unavailable, "connecting");
  assert.equal(deriveView({ ...base, wanted: true, permissionRefused: true, targetsStatus: "failed" }).unavailable, "permission");
  assert.deepEqual(deriveView({ ...base, wanted: true, targetsStatus: "ok", noScreen: "no_display" }).unavailable, { noScreen: "no_display" });
  assert.equal(formatLive(42_000), "0:42");
  assert.equal(measureFps([0, 500, 1000, 1500, 2000, 2500], 2500), 2);
  assert.equal(measureFps([2400], 2500), null);
});

test("the gradient keeps a white bot tinted and mid-light, so it never washes out", () => {
  const white = hexToOklch("#f4f4f5");
  assert.ok(white && white.c < NEUTRAL_CHROMA && white.l > 0.9);
  const base = gradientBase("#f4f4f5");
  assert.ok(base.c >= 0.04, "a neutral colour is given a tint");
  assert.ok(base.l <= 0.82 && base.l >= 0.5, "lightness stays in the middle band");
  const dark = gradientBase("#000000");
  assert.ok(dark.l >= 0.5);
  // a chromatic colour keeps its hue
  const blue = gradientBase("#3b82f6");
  assert.ok(Math.abs(blue.h - hexToOklch("#3b82f6")!.h) < 0.01);
  const vars = gradientVars("#8b9cff");
  assert.match(vars["--bot"], /^oklch\(/);
  assert.match(vars["--bot2"], /calc\(h \+ 48\)/);
  assert.match(vars["--bot3"], /calc\(l - \.22\)/);
});
