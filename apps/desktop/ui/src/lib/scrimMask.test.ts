import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { contrastRatio, deriveTheme, mix, paletteById } from "ui/theme-model";
import { legibleAlpha, TEXT_TARGET } from "ui/surface-model";
import { SCRIM_FEATHER, SCRIM_CONTENT_MAX, scrimCoverageX, scrimPlateauHalf, textExtent } from "./scrimMask.ts";

const css = readFileSync(new URL("../App.module.css", import.meta.url), "utf8");
const WIDTHS = [260, 320, 414, 560, 760, 900, 1000, 1140, 1400, 2000];

test("text never sits on a feather: full scrim across the whole reading column at every pane width", () => {
  for (const w of WIDTHS) {
    const [left, right] = textExtent(w);
    for (let x = left; x <= right; x += 1) {
      assert.equal(scrimCoverageX(w, x), 1, `pane ${w}px, text at x=${x.toFixed(0)}`);
    }
  }
});

test("a narrow pane gets a full-width scrim, a wide one a soft column edge outside the plateau", () => {
  assert.equal(scrimCoverageX(320, 0), 1);
  assert.equal(scrimCoverageX(320, 320), 1);
  const w = 1400;
  const half = scrimPlateauHalf(w);
  assert.equal(half, SCRIM_CONTENT_MAX / 2 + 24);
  assert.equal(scrimCoverageX(w, w / 2 + half + SCRIM_FEATHER / 2), 0.5);
  assert.equal(scrimCoverageX(w, w - 1), 0, "far from the column nothing is painted");
});

test("the review's numbers: the old geometry (feather inside the pane) halves the scrim under text at 320 px", () => {
  // Old: plateau = min(50% - feather, 400); fade over the feather.
  const oldCoverage = (paneW: number, x: number) => {
    const half = Math.min(paneW / 2 - SCRIM_FEATHER, 400);
    const d = Math.abs(x - paneW / 2);
    return d <= half ? 1 : Math.max(0, 1 - (d - half) / SCRIM_FEATHER);
  };
  const d = deriveTheme(paletteById("obsidian")!);
  const alpha = legibleAlpha(d.text, d.bg, "#ffffff", TEXT_TARGET); // Obsidian over white: 0.75
  const at = (cover: number) => contrastRatio(d.text, mix("#ffffff", d.bg, alpha * cover));
  const [left] = textExtent(320);
  assert.ok(at(1) >= TEXT_TARGET, "full scrim: 7:1");
  assert.ok(oldCoverage(320, left + 2) <= 0.5, `old mask under the first text column: ${oldCoverage(320, left + 2).toFixed(2)}`);
  assert.ok(at(oldCoverage(320, left + 2)) < 3, "which is the review's ~2:1");
  assert.equal(scrimCoverageX(320, left + 2), 1, "the new mask keeps the full alpha there");
});

test("App.module.css paints exactly the geometry lib/scrimMask.ts tests", () => {
  assert.match(css, /--scrim-half: min\(50%, calc\(var\(--size-content-max\) \/ 2 \+ var\(--space-6\)\)\);/);
  assert.match(css, /--scrim-feather: var\(--space-16\);/);
  assert.match(css, /transparent calc\(50% - var\(--scrim-half\) - var\(--scrim-feather\)\),\s*var\(--color-text\) calc\(50% - var\(--scrim-half\)\),\s*var\(--color-text\) calc\(50% \+ var\(--scrim-half\)\),\s*transparent calc\(50% \+ var\(--scrim-half\) \+ var\(--scrim-feather\)\)/);
  // No fade may start inside the plateau.
  assert.doesNotMatch(css, /--scrim-half: min\(calc\(50% - var\(--scrim-feather\)\)/);
});
