import assert from "node:assert/strict";
import { test } from "node:test";
import {
  FLOAT_GAP,
  FLOAT_PAD,
  fitRail,
  isFloatShell,
  MAIN_MIN,
  normalizeShellLayout,
  railAfterStep,
  panelMax,
  panelWidthFromPointer,
  railTier,
  railWidthFromPointer,
  RAIL_MIN,
  RAIL_MINI,
  RAIL_NARROW_BELOW,
  stepPanel,
  stepRail
} from "./shellLayout.ts";

test("dragging the rail below ~150px collapses it to icons only", () => {
  assert.equal(railWidthFromPointer(149), RAIL_MINI);
  assert.equal(railWidthFromPointer(10), RAIL_MINI);
  assert.equal(railWidthFromPointer(150), 200);
  assert.equal(railWidthFromPointer(320), 320);
  assert.equal(railWidthFromPointer(900), 440);
});

test("the outer gap of the floating layout is subtracted", () => {
  assert.equal(railWidthFromPointer(320, 12), 308);
  assert.equal(railWidthFromPointer(160, 12), RAIL_MINI);
});

test("panel width is measured from the right edge and clamped", () => {
  assert.equal(panelWidthFromPointer(1000, 1400), 400);
  assert.equal(panelWidthFromPointer(1300, 1400), 260);
  assert.equal(panelWidthFromPointer(100, 1400), panelMax(1400));
  assert.equal(panelMax(1400), 840);
  assert.equal(panelMax(300), 260);
});

test("keyboard steps move in 16px and respect the snap", () => {
  assert.equal(stepRail(264, -1), 248);
  assert.equal(stepRail(200, -1), RAIL_MINI);
  assert.equal(stepRail(RAIL_MINI, 1), 200);
  assert.equal(stepRail(440, 1), 440);
  assert.equal(stepPanel(320, 1, 1400), 336);
  assert.equal(stepPanel(260, -1, 1400), 260);
});

test("stored layouts are validated", () => {
  assert.deepEqual(normalizeShellLayout(null), { rail: 264, panel: 320 });
  assert.deepEqual(normalizeShellLayout({ rail: "wide", panel: NaN }), { rail: 264, panel: 320 });
  assert.deepEqual(normalizeShellLayout({ rail: 70, panel: 100 }), { rail: RAIL_MINI, panel: 260 });
  assert.deepEqual(normalizeShellLayout({ rail: 9000, panel: 9000 }), { rail: 440, panel: 1600 });
  assert.deepEqual(normalizeShellLayout({ rail: 310.4, panel: 400 }), { rail: 310, panel: 400 });
});

test("rail tiers decide what the apps pill shows", () => {
  assert.equal(railTier(RAIL_MINI), "mini");
  assert.equal(railTier(200), "narrow");
  assert.equal(railTier(264), "medium");
  assert.equal(railTier(300), "large");
  assert.equal(railTier(379), "large");
  assert.equal(railTier(380), "wide");
  assert.equal(railTier(440), "wide");
});

test("the panel leaves the chat room next to a wide rail", () => {
  assert.equal(panelMax(900, 440), 260); // 900 - 440 - 320 < 260: the floor wins
  assert.equal(panelMax(1200, 264), 616);
  assert.equal(panelMax(1400, 264), 816);
  assert.equal(panelWidthFromPointer(0, 1200, 0, 264), 616);
  assert.equal(stepPanel(616, 1, 1200, 264), 616);
});

test("the float chrome (padding and gaps) is subtracted so the chat keeps MAIN_MIN", () => {
  assert.equal(FLOAT_PAD, 12);
  assert.equal(FLOAT_GAP, 12);
  // Not floating: unchanged by the new argument.
  assert.equal(panelMax(1400, 264), 816);
  assert.equal(panelMax(1400, 264, false), 816);
  // Floating: 2 x 12 padding + 2 x 12 gaps = 48px less room for the panel.
  assert.equal(panelMax(1400, 264, true), 768);
  const vw = 1100;
  const panel = panelMax(vw, 264, true);
  assert.ok(vw - 48 - 264 - panel >= MAIN_MIN, "chat column keeps MAIN_MIN next to rail and panel");
  assert.equal(panelWidthFromPointer(0, 1400, 12, 264, true), 768);
  assert.equal(stepPanel(760, 1, 1400, 264, true), 768);
});

test("a rail that would squeeze the chat is narrowed, then snaps to icons", () => {
  assert.equal(isFloatShell("glass", false), true);
  assert.equal(isFloatShell("solid", true), true);
  assert.equal(isFloatShell("line", false), false);
  assert.equal(fitRail(1400, 264, 320, true, true), 264);
  assert.equal(fitRail(900, 440, 260, true, true), 900 - 48 - 260 - MAIN_MIN);
  assert.ok(fitRail(900, 440, 260, true, true) >= RAIL_MIN);
  assert.equal(fitRail(760, 264, 260, true, true), RAIL_MINI);
  assert.equal(fitRail(760, 264, 260, false, true), RAIL_MINI);
  // Closed panel at a normal width: only the rail and the chat share the row.
  assert.equal(fitRail(900, 264, 260, true, false), 264);
  assert.equal(fitRail(900, 440, 260, true, false), 440);
});

test("below RAIL_NARROW_BELOW the rail is icons only, panel open or not", () => {
  assert.equal(fitRail(RAIL_NARROW_BELOW - 1, 264, 260, true, false), RAIL_MINI);
  assert.equal(fitRail(760, 440, 260, false, false), RAIL_MINI);
  assert.equal(fitRail(RAIL_NARROW_BELOW, 264, 260, true, false), 264);
});

test("keyboard steps start from the displayed (fitted) rail, not the stored one", () => {
  // 900 px, float, stored 440, panel 260: displayed 272 (room), as in the review.
  const fitted = (c: number) => fitRail(900, c, 260, true, true);
  assert.equal(fitted(440), 900 - 48 - 260 - MAIN_MIN);
  const displayed = fitted(440);
  // Narrowing moves what the user sees and becomes the stored preference.
  assert.equal(railAfterStep(440, displayed, -1, fitted), displayed - 16);
  // Widening past what fits has no visible effect: the stored preference is left alone.
  assert.equal(railAfterStep(440, displayed, 1, fitted), 440);
  // Mini state shown because nothing fits: widening cannot show anything, stored stays.
  const fittedTight = (c: number) => fitRail(760, c, 260, true, true);
  assert.equal(fittedTight(264), RAIL_MINI);
  assert.equal(railAfterStep(264, RAIL_MINI, 1, fittedTight), 264);
  // A rail the user collapsed, with room: widening expands it.
  const roomy = (c: number) => fitRail(1400, c, 320, true, true);
  assert.equal(railAfterStep(RAIL_MINI, RAIL_MINI, 1, roomy), RAIL_MIN);
});
