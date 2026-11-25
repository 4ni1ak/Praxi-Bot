import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

/* The shell chrome contract (design wave 2), checked on the shipped sources: no DOM in node --test. */
const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
const app = read("../App.tsx");
const css = read("../App.module.css");
const appsPillCss = read("../components/AppsPill.module.css");
const sidebarCss = read("../../../../../packages/ui/src/components/Sidebar.module.css");
const block = (source: string, selector: string) => {
  const start = source.indexOf(`${selector} {`);
  return start < 0 ? "" : source.slice(start, source.indexOf("}", start));
};

test("the title bar has exactly one place picker: the left 'This computer' pill is gone", () => {
  assert.equal((app.match(/<ComputerPicker\b/g) ?? []).length, 1);
  assert.doesNotMatch(app, /runtimeLabel|runtimeName|account\.localComputer/);
  assert.doesNotMatch(css, /\.runtimeLabel|\.runtimeName|\.liveDot/);
  assert.match(app, /className=\{styles\.titleLead\} id=\{TITLEBAR_LEAD_ID\}/);
});

test("the title bar keeps the height the overlay panel is offset by (Sidebar.module.css)", () => {
  assert.match(block(css, ".titleBar"), /min-height:\s*calc\(var\(--space-12\) \+ var\(--space-1\)\)/);
  assert.match(sidebarCss, /top:\s*calc\(var\(--titlebar-h, var\(--space-0\)\) \+ var\(--space-12\) \+ var\(--space-1\)\)/);
});

test("rail rows: selected is the accent state layer with a straight marker, not a curved inset edge", () => {
  assert.match(css, /\.rail \.railItem\[aria-current="page"\][^{]*\{[^}]*background-color:\s*var\(--state-selected\)/);
  assert.match(css, /\.rail \.railItem\[aria-current="page"\]::before/);
  assert.doesNotMatch(css, /box-shadow:\s*inset var\(--size-border-thick\) 0 0/);
  assert.match(block(css, ".rail .railItem"), /border-radius:\s*var\(--radius-md\)/);
});

test("the Apps row always carries its label; the wordmark steps aside on a narrow rail", () => {
  assert.doesNotMatch(block(appsPillCss, ".label"), /display:\s*none/);
  assert.match(css, /\[data-rail-tier="narrow"\]\) \.brandName \{\s*display:\s*none/);
});
