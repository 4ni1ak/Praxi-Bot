import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { APP_FNS, evaluateShell } from "./shellGeometry.ts";
import {
  parseDurationMs,
  reserveActive,
  reserveDelayMs,
  RESERVE_FALLBACK_MS,
  RESERVE_MARGIN_MS
} from "./shellLayout.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
const sources = {
  sidebarCss: read("../../../../../packages/ui/src/components/Sidebar.module.css"),
  tokensCss: read("../../../../../packages/ui/tokens.css"),
  globalCss: read("../global.css"),
  themeTs: read("../../../../../packages/ui/src/theme.ts"),
  sidebarTsx: read("../../../../../packages/ui/src/components/Sidebar.tsx")
};
const withSlow = (value: string) => ({
  ...sources,
  tokensCss: sources.tokensCss.replace("--motion-duration-slow: 450ms;", `--motion-duration-slow: ${value};`)
});

test("CSS durations parse with units and the reserve follows them", () => {
  assert.equal(parseDurationMs("450ms"), 450);
  assert.equal(parseDurationMs("0.45s"), 450);
  assert.equal(parseDurationMs("1s"), 1000);
  assert.equal(parseDurationMs(" 600ms "), 600);
  for (const bad of ["450", "calc(1s)", "var(--x)", "-1s", "1m", "", undefined]) {
    assert.equal(parseDurationMs(bad), undefined);
  }
  assert.equal(reserveDelayMs(450, false), 450 + RESERVE_MARGIN_MS);
  assert.equal(reserveDelayMs(parseDurationMs("0.6s"), false), reserveDelayMs(parseDurationMs("600ms"), false));
  assert.equal(reserveDelayMs(450, true), 0, "reduced motion: no transition, no delay");
  assert.equal(reserveDelayMs(0, false), 0);
  assert.equal(reserveDelayMs(undefined, false), RESERVE_FALLBACK_MS);
  assert.equal(reserveActive(true, 9999, 500), true);
  assert.equal(reserveActive(false, 499, 500), true);
  assert.equal(reserveActive(false, 500, 500), false);
});

test("the reserve follows the slide token in ms and s alike; a fixed 500 ms reserve does not", () => {
  assert.notEqual(withSlow("0.6s").tokensCss, sources.tokensCss);
  const ms = evaluateShell(withSlow("600ms"), APP_FNS);
  const s = evaluateShell(withSlow("0.6s"), APP_FNS);
  assert.deepEqual(ms.violations, []);
  assert.deepEqual(s.violations, []);
  assert.deepEqual(s.contractFailures, ms.contractFailures);
  // A reserve hard-coded to 500 ms covers 450 ms, but not 1 s, whichever unit the token uses.
  const fixed500 = { reserveDelay: () => 500 };
  assert.deepEqual(evaluateShell(sources, APP_FNS, fixed500).violations, []);
  const tooShort = evaluateShell(withSlow("1s"), APP_FNS, fixed500);
  assert.ok(tooShort.violations.some((v) => /closing t=\d+ms .*\(transition\): main/.test(v)));
  assert.deepEqual(evaluateShell(withSlow("1000ms"), APP_FNS, fixed500).violations, tooShort.violations);
  // The derived reserve covers it.
  assert.deepEqual(evaluateShell(withSlow("1s"), APP_FNS).violations, []);
});

test("an unreadable slide token or a transition that ignores it is rejected", () => {
  assert.ok(evaluateShell(withSlow("calc(1s)"), APP_FNS).contractFailures.some((f) => /--motion-duration-slow/.test(f)));
  assert.ok(evaluateShell(withSlow("450"), APP_FNS).contractFailures.some((f) => /--motion-duration-slow/.test(f)));
  const css = sources.sidebarCss.replace("width var(--motion-duration-slow)", "width var(--motion-duration-normal)");
  assert.notEqual(css, sources.sidebarCss);
  assert.ok(evaluateShell({ ...sources, sidebarCss: css }, APP_FNS).contractFailures.some((f) => /\.panel transition width/.test(f)));
  const noReduced = sources.globalCss.replace("prefers-reduced-motion: reduce", "prefers-reduced-motion: none");
  assert.notEqual(noReduced, sources.globalCss);
  assert.ok(evaluateShell({ ...sources, globalCss: noReduced }, APP_FNS).contractFailures.some((f) => /prefers-reduced-motion/.test(f)));
});

test("an unrelated at-rule passes; one holding a shell rule names the at-rule, file:line and the remedy", () => {
  const harmless = `${sources.sidebarCss}\n@media (max-height: 600px) { .footer { opacity: 0.9; } }\n`;
  assert.deepEqual(evaluateShell({ ...sources, sidebarCss: harmless }, APP_FNS).contractFailures, []);
  const bad = `${sources.sidebarCss}\n@media (min-width: 1px) { .panelInner { width: 0px; } }\n`;
  const failures = evaluateShell({ ...sources, sidebarCss: bad }, APP_FNS).contractFailures.join("\n");
  assert.match(failures, /Sidebar\.module\.css:\d+ :: at-rule "@media \(min-width: 1px\)" contains a rule for the contracted shell selector \.panelInner/);
  assert.match(failures, /move the rule out of the at-rule, or extend shellContract\.ts/);
});
