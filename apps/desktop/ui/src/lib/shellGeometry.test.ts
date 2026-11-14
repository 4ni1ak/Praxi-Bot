import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { APP_FNS, evaluateShell, shellBoxes } from "./shellGeometry.ts";
import { LEGACY_FNS, LEGACY_SIDEBAR_CSS } from "./shellLegacy.ts";
import { evalLength, readShellContract } from "./shellContract.ts";
import { FLOAT_GAP, FLOAT_PAD } from "./shellLayout.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
const sources = {
  sidebarCss: read("../../../../../packages/ui/src/components/Sidebar.module.css"),
  tokensCss: read("../../../../../packages/ui/tokens.css"),
  globalCss: read("../global.css"),
  themeTs: read("../../../../../packages/ui/src/theme.ts")
};

test("the shipped shell passes the contract and the whole matrix", () => {
  const report = evaluateShell(sources, APP_FNS);
  assert.deepEqual(report.contractFailures, []);
  assert.deepEqual(report.violations, []);
  assert.ok(report.cells >= 600);
  assert.ok(report.narrowestMain.width >= 280);
});

test("the pre-fix CSS and clamp violate the clip and chat-width invariants", () => {
  const legacy = evaluateShell({ ...sources, sidebarCss: LEGACY_SIDEBAR_CSS }, LEGACY_FNS);
  const text = [...legacy.contractFailures, ...legacy.violations].join("\n");
  assert.match(text, /\.panelInner width/);
  assert.match(text, /panelInner\.right \d+ > panel content right \d+/);
  assert.match(text, /vw=760 .*main [\d.]+px < 280/);
});

test("the CSS contract alone catches an inner column as wide as the panel's border box", () => {
  const css = sources.sidebarCss.replace(/width: calc\(var\(--panel-w\) - var\(--panel-edge, var\(--space-0\)\)\);/, "width: var(--panel-w);");
  assert.notEqual(css, sources.sidebarCss);
  const report = evaluateShell({ ...sources, sidebarCss: css }, APP_FNS);
  assert.ok(report.contractFailures.some((f) => f.includes(".panelInner width")));
  assert.ok(report.violations.some((v) => v.includes("clipped")));
});

test("a wrong panel-edge value is a geometry violation even when the width rule looks fine", () => {
  const css = sources.sidebarCss.replace("--panel-edge: calc(var(--size-border) * 2);", "--panel-edge: var(--size-border);");
  assert.notEqual(css, sources.sidebarCss);
  const report = evaluateShell({ ...sources, sidebarCss: css }, APP_FNS);
  assert.deepEqual(report.contractFailures, []);
  assert.ok(report.violations.some((v) => v.includes("clipped 1px")));
});

test("boxes of the reported case: 1400 px, float, panel open", () => {
  const { model } = readShellContract(sources, { floatPad: FLOAT_PAD, floatGap: FLOAT_GAP });
  const g = shellBoxes({ vw: 1400, rail: 264, panel: 320, panelOpen: true, float: true }, model);
  assert.deepEqual(g.rail, { left: 12, right: 276 });
  assert.deepEqual(g.panel, { left: 1068, right: 1388 });
  assert.equal(g.panelContentRight, 1387);
  assert.equal(g.panelInner.right, 1387);
  assert.equal(g.main.right - g.main.left, 1068 - 12 - 288);
});

test("the length evaluator reads plain CSS lengths", () => {
  const vars = new Map([["--space-3", "12px"]]);
  assert.equal(evalLength("var(--space-3)", vars), 12);
  assert.equal(evalLength("calc(var(--space-3) * -1)", vars), -12);
  assert.equal(evalLength("calc(320px - var(--missing, 2px))", vars), 318);
  assert.equal(evalLength("calc(320px - var(--missing, var(--space-3)))", vars), 308);
  assert.equal(evalLength("100%", vars), undefined);
});
