import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { URL } from 'node:url';
import process from 'node:process';
import {
  PALETTES, FAVORITE_THEMES, DEFAULT_CUSTOM_THEME, UI_STYLES, buildTokens, contrastRatio, deriveTheme,
  fallbackAccent, inkOn, mix, normalizeHex, paletteById
} from '../src/themeModel.ts';
import {
  buildThemeTokens, effectiveUiStyle, planSurfaces, PANEL_BLUR_MAX, PANEL_BLUR_MIN, PANEL_TRANSPARENCY_MAX,
  PANEL_TRANSPARENCY_MIN, TEXT_TARGET, WORST_TONE
} from '../src/surfaceModel.ts';

// Every shipped theme must be readable.  The derivation pulls faded text, status colours and
// accent text toward the text colour until they pass, so the hard requirement is the one
// the derivation cannot repair: the theme's own text colour on bg / surface / raised.
const root = new URL('../', import.meta.url);
const css = readFileSync(new URL('tokens.css', root), 'utf8');
assert.equal(PALETTES.length, 29, 'the prototype ships 29 themes');
assert.equal(new Set(PALETTES.map((p) => p.id)).size, PALETTES.length, 'theme ids are unique');
for (const id of FAVORITE_THEMES) assert.ok(paletteById(id), `favourite ${id} exists`);

const BOT_COLOR_SAMPLES = ['#3b82f6', '#10b981', '#8b5cf6', '#f59e0b', '#e11d48', '#0ea5e9', '#ffffff', '#000000', '#808080'];
const failures = [];
const check = (name, label, fg, bg, min) => {
  const r = contrastRatio(fg, bg);
  if (r < min) failures.push(`${name}: ${label} ${r.toFixed(2)} < ${min}`);
  return r;
};

for (const theme of [...PALETTES, { id: 'custom (default)', ...DEFAULT_CUSTOM_THEME }]) {
  const d = deriveTheme(theme);
  const name = theme.id;
  const surfaces = { background: d.bg, surface: d.surface, raised: d.raised, hover: d.hover };
  let lowest = Infinity;
  for (const [sname, s] of Object.entries(surfaces)) {
    const required = sname === 'hover' ? 0 : 4.5; // text on bg / surface / raised is the contract
    const r = check(name, `text/${sname}`, d.text, s, required);
    if (sname !== 'hover') lowest = Math.min(lowest, r);
    for (const [label, color] of [['secondary', d.secondary], ['muted', d.muted], ['success', d.success], ['warning', d.warning], ['danger', d.danger]]) {
      check(name, `${label}/${sname}`, color, s, 4.5);
    }
    check(name, `focus/${sname}`, d.focus, s, sname === 'hover' ? 0 : 3);
    check(name, `control border/${sname}`, d.lineStrong, s, sname === 'hover' ? 0 : 3);
  }
  for (const style of UI_STYLES) {
    const t = buildTokens(theme, theme.accent ?? fallbackAccent(d), style, false);
    for (const key of ['--color-surface', '--color-surface-elevated', '--color-border']) assert.ok(t[key], `${name}/${style}: ${key}`);
  }
  // Accents: the theme's own, then arbitrary bot colours.
  for (const accent of [theme.accent ?? fallbackAccent(d), ...BOT_COLOR_SAMPLES]) {
    const t = buildTokens(theme, accent, 'solid', false);
    const a = normalizeHex(accent);
    check(name, `accent button ${a}`, inkOn(a), a, 4.5);
    check(name, `accent button hover ${a}`, inkOn(a), t['--color-accent-hover'], 4.5);
    for (const s of [d.bg, d.surface, d.raised]) check(name, `accent text ${a}`, t['--color-accent-text'], s, 4.5);
    for (const token of ['text', 'keyword', 'string', 'number', 'comment']) {
      check(name, `code ${token}`, t[`--color-code-${token === 'text' ? 'text' : token}`], d.bg, 4.5);
    }
  }
  check(name, 'danger button', d.dangerInk, d.danger, 4.5);
  for (const status of ['success', 'warning', 'danger']) {
    check(name, `${status} badge`, d[status], mix(d[status], d.surface, 0.9), 4.5);
  }
  process.stdout.write(`${name}: ${d.light ? 'light' : 'dark'}; text contrast >= ${lowest.toFixed(2)}:1 (bg/surface/raised)\n`);
}

// Translucent surfaces (card 244-E): every layer text can sit on, at the slider extremes, over the
// worst backdrop (pure white / black) and over mid-grey and coloured backdrops.  Primary text must
// reach TEXT_TARGET on the shell layers and 4.5 elsewhere; muted / secondary / status / accent text
// 4.5 everywhere; faint text and control borders 3.
const flat = (c) => ({ peak: { light: c, dark: c }, soft: { light: c, dark: c } });
const TONES = [['worst', WORST_TONE], ['grey', flat('#808080')], ['blue', flat('#4a7bd0')], ['orange', flat('#d9822b')],
  ['busy', { peak: { light: '#f5f0e0', dark: '#0a1020' }, soft: { light: '#9ab0a8', dark: '#203038' } }]];
let surfaceCells = 0;
for (const theme of [...PALETTES, { id: 'custom (default)', ...DEFAULT_CUSTOM_THEME }]) {
  const d = deriveTheme(theme);
  const accent = theme.accent ?? fallbackAccent(d);
  for (const style of UI_STYLES) {
    for (const transparentChat of [false, true]) {
      for (const transparency of [PANEL_TRANSPARENCY_MIN, PANEL_TRANSPARENCY_MAX]) {
        for (const blur of [PANEL_BLUR_MIN, PANEL_BLUR_MAX]) {
          for (const [toneName, tone] of TONES) {
            const opts = { transparency, blur, transparentChat, tone };
            const plan = planSurfaces(d, style, true, opts);
            const t = buildThemeTokens(theme, accent, style, true, opts);
            const cell = `${theme.id}/${style}/${transparentChat ? 'clear' : 'pane'}/t${transparency}/b${blur}/${toneName}`;
            for (const layer of plan.layers) {
              surfaceCells++;
              check(cell, `text/${layer.name}`, d.text, layer.color, layer.textMin);
              for (const key of ['secondary', 'muted']) check(cell, `${key}/${layer.name}`, t[`--color-text-${key}`], layer.color, 4.5);
              for (const key of ['success', 'warning', 'danger', 'accent-text']) check(cell, `${key}/${layer.name}`, t[`--color-${key}`], layer.color, 4.5);
              check(cell, `faint/${layer.name}`, t['--color-text-faint'], layer.color, 3);
              check(cell, `control border/${layer.name}`, t['--color-border-strong'], layer.color, 3);
            }
            // The re-derived colours must still hold on the theme's own solid surfaces.
            for (const s of [d.bg, d.surface, d.raised]) {
              check(cell, 'muted/solid', t['--color-text-muted'], s, 4.5);
              check(cell, 'accent text/solid', t['--color-accent-text'], s, 4.5);
            }
            for (const key of ['--shell-fill', '--shell-filter', '--center-fill', '--center-scrim', '--float-fill', '--composer-fill']) {
              assert.ok(t[key], `${cell}: ${key}`);
            }
            if (style === 'solid' && !transparentChat) {
              assert.equal(plan.layers.length, 0, `${cell}: solid panels add no translucent layer`);
            }
            if (transparentChat) assert.equal(t['--center-fill'], 'transparent', `${cell}: clear chat area has no fill`);
          }
        }
      }
    }
  }
}
// Platform fallbacks: reduced transparency paints solid; no backdrop-filter turns glass into tinted.
assert.equal(effectiveUiStyle('glass', { backdropFilter: true, reducedTransparency: true }), 'solid');
assert.equal(effectiveUiStyle('glass', { backdropFilter: false, reducedTransparency: false }), 'tinted');
assert.equal(effectiveUiStyle('tinted', { backdropFilter: false, reducedTransparency: false }), 'tinted');
process.stdout.write(`surfaces: ${surfaceCells} layer checks (styles x chat area x transparency/blur extremes x backdrops; text >= ${TEXT_TARGET}:1 on panels)
`);

// tokens.css must carry exactly the derived default theme (no stale generated CSS).
const def = paletteById('obsidian');
const expected = buildThemeTokens(def, fallbackAccent(deriveTheme(def)), 'solid', false);
for (const [k, v] of Object.entries(expected)) assert.ok(css.includes(`${k}: ${v};`), `tokens.css is stale: ${k} (run pnpm --dir packages/ui generate:themes)`);

if (failures.length) {
  process.stderr.write(`\n${failures.length} contrast violation(s):\n${failures.join('\n')}\n`);
  process.exit(1);
}
process.stdout.write(`\n${PALETTES.length} themes OK; accent / status / code / default-CSS checks passed\n`);
