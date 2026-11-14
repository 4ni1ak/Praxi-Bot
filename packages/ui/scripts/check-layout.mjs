import { readFileSync } from 'node:fs';
import { URL } from 'node:url';
import process from 'node:process';
import { evaluateShell, APP_FNS } from '../../../apps/desktop/ui/src/lib/shellGeometry.ts';
import { LEGACY_FNS, LEGACY_SIDEBAR_CSS } from '../../../apps/desktop/ui/src/lib/shellLegacy.ts';

// Deterministic v6 shell layout check (card 233).  No browser: the geometry functions are the
// ones the app uses, the border / padding numbers are read from the CSS.  Run with `--selftest`
// for the mutation test only; the default run does the real check and then the mutation test.
const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8');
const sources = {
  sidebarCss: read('../src/components/Sidebar.module.css'),
  tokensCss: read('../tokens.css'),
  globalCss: read('../../../apps/desktop/ui/src/global.css'),
  themeTs: read('../src/theme.ts')
};

let failed = false;
const selftestOnly = process.argv.includes('--selftest');

if (!selftestOnly) {
  const report = evaluateShell(sources, APP_FNS);
  const problems = [...report.contractFailures, ...report.violations];
  process.stdout.write(`layout matrix: ${report.cells} cells; narrowest chat column ${report.narrowestMain.width.toFixed(1)}px (${report.narrowestMain.cell})\n`);
  if (problems.length) {
    failed = true;
    process.stderr.write(`layout check FAILED (${report.contractFailures.length} contract, ${report.violations.length} geometry):\n`);
    for (const p of problems) process.stderr.write(`  - ${p}\n`);
  } else {
    process.stdout.write('layout contract and matrix: ok\n');
  }
}

// Mutation test: the pre-fix CSS and clamp must be caught, for the reasons the card names.
const legacy = evaluateShell({ ...sources, sidebarCss: LEGACY_SIDEBAR_CSS }, LEGACY_FNS);
const text = [...legacy.contractFailures, ...legacy.violations].join('\n');
const expectations = [
  ['contract rule .panelInner width', /\.panelInner width/],
  ['panelInner clipped by the panel border', /panelInner\.right .* > panel content right/],
  ['chat column below 280 at 760', /vw=760 .*main [\d.]+px < 280/]
];
for (const [label, re] of expectations) {
  if (!re.test(text)) {
    failed = true;
    process.stderr.write(`selftest FAILED: the legacy shell was not caught for "${label}"\n`);
  }
}
if (!failed) process.stdout.write(`selftest: legacy shell rejected (${legacy.contractFailures.length} contract, ${legacy.violations.length} geometry violations)\n`);
process.exitCode = failed ? 1 : 0;
