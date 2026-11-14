// Bounded headless-Chrome layout run (card 233).  Local and optional, not part of CI.
//   node scripts/check-layout-browser.mjs [--require] [--only=<n,n|substring>] [--budget=<seconds>]
// Builds the mock bundle, serves it with `vite preview`, opens each case in a fresh headless Chrome
// with `--dump-dom` and reads the JSON the dev-only layout probe appended.  Everything it creates
// lives in one temp dir that is deleted in `finally`; Chrome and vite are killed as process trees.
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import process from 'node:process';
import { fileURLToPath, URL } from 'node:url';
import { clearTimeout, setTimeout } from 'node:timers';
import { setTimeout as delay } from 'node:timers/promises';

const args = process.argv.slice(2);
const flag = (name) => args.find((a) => a.startsWith(`--${name}=`))?.split('=')[1];
const mustRun = args.includes('--require');
const only = flag('only');
const budgetMs = Number(flag('budget') ?? 480) * 1000;
const CASE_TIMEOUT = 20000;
const root = fileURLToPath(new URL('../', import.meta.url));
const CHROMES = [process.env.CHROME_BIN, 'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe', '/usr/bin/google-chrome', '/usr/bin/chromium']
  .filter(Boolean);
const chrome = CHROMES.find((p) => existsSync(p));
if (!chrome) {
  process.stdout.write('SKIPPED: no Chrome found (set CHROME_BIN)\n');
  process.exit(mustRun ? 1 : 0);
}

// 24 cases: widths x style x theme (18), plus 6 specials.
const cases = [];
for (const width of [760, 1400, 1920]) {
  for (const style of ['solid', 'glass', 'line']) {
    for (const theme of ['clay', 'obsidian']) {
      cases.push({ name: `${width}/${style}/${theme}/open`, width, query: `theme=${theme}&style=${style}&panel=1&rail=264` });
    }
  }
}
cases.push(
  { name: '1400/glass/clay/closed', width: 1400, query: 'theme=clay&style=glass&rail=264' },
  { name: '1400/glass/clay/open/stored1600', width: 1400, query: 'theme=clay&style=glass&panel=1&rail=264&stored=1600' },
  { name: '1400/glass/clay/open/corners=round', width: 1400, query: 'theme=clay&style=glass&panel=1&rail=264&corners=round' },
  { name: '1400/solid/clay/open/backdrop', width: 1400, query: 'theme=clay&style=solid&panel=1&rail=264&backdrop=aurora' },
  { name: '1400/glass/obsidian/open/backdrop', width: 1400, query: 'theme=obsidian&style=glass&panel=1&rail=264&backdrop=aurora' },
  { name: '900/glass/clay/open/rail440', width: 900, query: 'theme=clay&style=glass&panel=1&rail=440' }
);
const selected = !only ? cases
  : /^[\d,]+$/.test(only) ? only.split(',').map((i) => cases[Number(i)]).filter(Boolean)
  : cases.filter((c) => c.name.includes(only));

const EPS = 0.5;
/** Same invariants as the pure model, applied to measured rects. */
function violations(p, open) {
  const out = [];
  const vw = p.viewport.width;
  const boxes = { rail: p.rail, main: p.main, panel: open ? p.panel : null, panelInner: open ? p.panelInner : null };
  for (const [k, b] of Object.entries(boxes)) {
    if (b && (b.left < -EPS || b.right > vw + EPS)) out.push(`${k} outside viewport (${b.left}..${b.right} of ${vw})`);
  }
  if (p.document.scrollWidth > p.document.clientWidth + EPS) out.push(`document scrolls horizontally (${p.document.scrollWidth} > ${p.document.clientWidth})`);
  if (p.main && p.rail && p.rail.right > p.main.left + EPS) out.push('rail overlaps main');
  if (open && p.panel && p.main && p.main.right > p.panel.left + EPS) out.push('main overlaps panel');
  if (p.main && vw >= 900 && p.main.width < 320 - EPS) out.push(`main ${p.main.width.toFixed(1)} < 320`);
  if (p.main && p.main.width < 280 - EPS) out.push(`main ${p.main.width.toFixed(1)} < 280`);
  if (open && p.panel) {
    const pad = p.float === 'on' ? 12 : 0;
    if (Math.abs(p.panel.right - (vw - pad)) > EPS) out.push(`panel.right ${p.panel.right} != ${vw - pad}`);
    if (p.panelInner && p.panelInner.right > p.panelContentRight + EPS) {
      out.push(`panelInner.right ${p.panelInner.right} > panel content right ${p.panelContentRight} (clipped ${(p.panelInner.right - p.panelContentRight).toFixed(1)}px)`);
    }
    if (p.panelRightMostDescendant && p.panelRightMostDescendant.right > p.panelContentRight + EPS) {
      out.push(`panel descendant <${p.panelRightMostDescendant.tag}> right ${p.panelRightMostDescendant.right} > ${p.panelContentRight}`);
    }
    if (p.panelInnerScroll && p.panelInnerScroll.scrollWidth > p.panelInnerScroll.clientWidth + EPS) {
      out.push(`panelInner scrolls horizontally (${p.panelInnerScroll.scrollWidth} > ${p.panelInnerScroll.clientWidth})`);
    }
  }
  return out;
}

const killTree = (pid) => {
  if (!pid) return;
  if (process.platform === 'win32') spawnSync('taskkill', ['/PID', String(pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
  else { try { process.kill(pid, 'SIGKILL'); } catch { /* already gone */ } }
};
const freePort = () => new Promise((ok, fail) => {
  const s = createServer(); s.unref(); s.on('error', fail);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => ok(port)); });
});
const run = (cmd, argv, opts) => new Promise((ok) => {
  const child = spawn(cmd, argv, { windowsHide: true, stdio: opts.stdio });
  let out = '';
  child.stdout?.on('data', (d) => { out += d; });
  const timer = setTimeout(() => { killTree(child.pid); }, opts.timeout);
  child.on('exit', () => { clearTimeout(timer); killTree(child.pid); ok(out); });
});

const tmp = mkdtempSync(join(tmpdir(), 'praxi-layout-'));
const started = Date.now();
let preview;
let failed = 0;
try {
  const vite = resolve(root, 'node_modules/vite/bin/vite.js');
  const dist = join(tmp, 'dist');
  const build = spawnSync(process.execPath, [vite, 'build', '--outDir', dist, '--emptyOutDir'],
    { cwd: root, env: { ...process.env, VITE_MOCK: '1' }, encoding: 'utf8', windowsHide: true, timeout: 120000 });
  if (build.status !== 0) throw new Error(`vite build failed:\n${build.stdout}\n${build.stderr}`);
  const port = await freePort();
  preview = spawn(process.execPath, [vite, 'preview', '--outDir', dist, '--port', String(port), '--strictPort', '--host', '127.0.0.1'],
    { cwd: root, windowsHide: true, stdio: 'ignore' });
  for (let n = 0; n < 50; n++) {
    try { if ((await globalThis.fetch(`http://127.0.0.1:${port}/`)).ok) break; } catch { /* not up yet */ }
    await delay(200);
  }
  let narrowest = Infinity;
  for (const [i, c] of selected.entries()) {
    if (Date.now() - started > budgetMs) { process.stdout.write(`BUDGET exhausted after ${i} cases\n`); failed++; break; }
    let out = '';
    for (let attempt = 0; attempt < 4 && !out.includes('layout-probe'); attempt++) {
      out = await run(chrome, ['--headless=new', '--disable-gpu', '--no-sandbox', '--hide-scrollbars',
      '--force-device-scale-factor=1', `--window-size=${c.width},900`, `--virtual-time-budget=${process.env.VTB ?? 20000}`,
      `--user-data-dir=${join(tmp, `profile-${i}-${attempt}`)}`, '--dump-dom', `http://127.0.0.1:${port}/?mock=1&probe=layout&${c.query}`],
      { stdio: ['ignore', 'pipe', args.includes('--debug') ? 'inherit' : 'ignore'], timeout: CASE_TIMEOUT });
    }
    const m = /<script id="layout-probe" type="application\/json">([\s\S]*?)<\/script>/.exec(out);
    if (!m && args.includes('--debug')) process.stdout.write(`DOM[${out.length}] probe=${out.includes("layout-probe")} pr=${/data-probe="[^"]*"/.exec(out)?.[0]} err=${/data-probe-error="[^"]*"/.exec(out)?.[0]} shell=${out.includes("data-shell-part")}
`);
    if (!m) { process.stdout.write(`FAIL ${c.name}: no probe output (timeout or crash)\n`); failed++; continue; }
    const p = JSON.parse(m[1].replace(/&quot;/g, '"').replace(/&amp;/g, '&'));
    const open = !c.name.includes('closed');
    const v = violations(p, open);
    const clip = open && p.panelInner ? p.panelInner.right - p.panelContentRight : 0;
    narrowest = Math.min(narrowest, p.main ? p.main.width : Infinity);
    const f = (b) => (b ? `${b.left.toFixed(1)}..${b.right.toFixed(1)}(w${b.width.toFixed(1)})` : '-');
    process.stdout.write(`${v.length ? 'FAIL' : 'ok  '} ${c.name} vp=${p.viewport.width} float=${p.float} rail=${f(p.rail)} main=${f(p.main)} panel=${f(p.panel)} borders=${JSON.stringify(p.panelBorders)} inner=${f(p.panelInner)} contentRight=${p.panelContentRight} clip=${clip.toFixed(1)} doc=${p.document.scrollWidth}/${p.document.clientWidth}\n`);
    for (const line of v) process.stdout.write(`       - ${line}\n`);
    if (v.length) failed++;
  }
  process.stdout.write(`${selected.length} cases, ${failed} failing, narrowest main ${narrowest.toFixed(1)}px, ${((Date.now() - started) / 1000).toFixed(0)}s\n`);
} catch (e) {
  failed++;
  process.stdout.write(`ERROR ${e.message}\n`);
} finally {
  killTree(preview?.pid);
  await delay(300);
  try { rmSync(tmp, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 }); } catch { /* best effort */ }
}
process.exit(failed ? 1 : 0);
