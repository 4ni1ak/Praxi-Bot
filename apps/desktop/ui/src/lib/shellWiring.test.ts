import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

/**
 * Wiring guard (card 233): the pure helpers are tested elsewhere; these assertions fail when
 * App.tsx stops using them (float chrome, the fitted rail, the displayed width, the settle and
 * window-resize hooks), which no helper-level test can see.
 */
const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
const app = read("../App.tsx");

test("App derives float from the settings and passes it to every panel clamp", () => {
  assert.match(app, /const float = isFloatShell\(settings\.uiStyle, backdropIsActive\(settings\.backdrop, settings\.backgroundUrl\)\);/);
  assert.match(app, /const panelWidth = Math\.min\(layout\.panel, panelMax\(viewportWidth, layout\.rail, float\)\);/);
  assert.match(app, /max=\{panelMax\(viewportWidth, layout\.rail, float\)\}/);
  assert.match(app, /panelWidthFromPointer\(x, window\.innerWidth, gridPad\(\), layout\.rail, float\)/);
  assert.match(app, /stepPanel\(panelWidth, dir === -1 \? 1 : -1, window\.innerWidth, layout\.rail, float\)/);
  assert.match(app, /FLOAT_PAD/);
});

test("App renders the FITTED rail width, reserved while the panel settles", () => {
  assert.match(app, /const panelReserve = usePanelReserve\(panelVisible\);/);
  assert.match(app, /const railWidth = fitRail\(viewportWidth, layout\.rail, panelWidth, float, panelReserve\);/);
  assert.match(app, /"--size-sidebar-width": `\$\{railWidth\}px`/);
  assert.match(app, /data-rail-tier=\{railTier\(railWidth\)\}/);
  assert.match(app, /const mini = isMiniRail\(railWidth\);/);
  assert.match(app, /value=\{railWidth\}/);
  assert.match(app, /const viewportWidth = useViewportWidth\(\);/);
});

test("rail keyboard steps start from the displayed width; Search uses the plan", () => {
  assert.match(app, /onStep=\{\(dir\) => setRail\(railAfterStep\(layout\.rail, railWidth, dir, fitStored\)\)\}/);
  assert.match(app, /const search = searchPlan\(layout\.rail, railWidth, botSearch !== null\);/);
  assert.match(app, /if \(search\.expandStored\) setRail\(RAIL_MIN\); setBotSearch\(search\.open \? "" : null\);/);
  assert.match(app, /botSearch !== null && \(!mini \|\| search\.popover\)/);
});

test("the stored rail width is only read where the fit or an explicit resize needs it", () => {
  let rest = app;
  const allowed = [
    /panelMax\(viewportWidth, layout\.rail, float\)/g,
    /fitRail\(viewportWidth, layout\.rail,/g,
    /fitRail\(viewportWidth, candidate,/g,
    /gridPad\(\), layout\.rail, float\)/g,
    /window\.innerWidth, layout\.rail, float\)/g,
    /railAfterStep\(layout\.rail, railWidth,/g,
    /searchPlan\(layout\.rail, railWidth,/g,
    /layout\.rail === rail/g,
    /layoutRef/g
  ];
  for (const re of allowed) rest = rest.replace(re, "");
  assert.doesNotMatch(rest, /layout\.rail/, "every other use of layout.rail would render or step the stored (unfitted) width");
});

test("the hooks exist and keep their contract", () => {
  const reserve = read("../state/usePanelReserve.ts");
  assert.match(reserve, /PANEL_SETTLE_MS/);
  assert.match(reserve, /return open \|\| settling;/);
  const viewport = read("../state/useViewportWidth.ts");
  assert.match(viewport, /dataset\.windowResizing = "true"/);
  assert.match(viewport, /delete root\.dataset\.windowResizing/);
});
