import assert from "node:assert/strict";
import { test } from "node:test";
import {
  buildThemeTokens,
  DEFAULT_SURFACE,
  effectiveUiStyle,
  GLASS_SATURATE,
  legibleAlpha,
  parsePanelBlur,
  parsePanelTransparency,
  parseTransparentChat,
  planSurfaces,
  PANEL_BLUR_MAX,
  PANEL_BLUR_MIN,
  TEXT_TARGET,
  WORST_TONE,
  type BackdropTone
} from "ui/surface-model";
import { contrastRatio, deriveTheme, mix, paletteById, UI_STYLES } from "ui/theme-model";
import { isFloatShell } from "./shellLayout.ts";

const THEMES = ["obsidian", "cobalt", "raven", "clay"] as const;
const theme = (id: string) => {
  const t = paletteById(id);
  assert.ok(t, id);
  return t;
};
const flat = (c: string): BackdropTone => ({ peak: { light: c, dark: c }, soft: { light: c, dark: c } });
const alphaOf = (fill: string) => Number(/ (\d+)%, transparent\)$/.exec(fill)?.[1] ?? 100) / 100;

test("token resolution: every theme x style resolves the shell, center and floating surfaces", () => {
  for (const id of THEMES) {
    for (const style of UI_STYLES) {
      const t = buildThemeTokens(theme(id), "#3b82f6", style, true, { ...DEFAULT_SURFACE, blur: 30 });
      const d = deriveTheme(theme(id));
      assert.equal(t["--shell-filter"], style === "glass" ? `blur(30px) saturate(${GLASS_SATURATE})` : "none", `${id}/${style} filter`);
      assert.equal(t["--center-filter"], t["--shell-filter"], `${id}/${style}: the center pane is one of the shell panels`);
      if (style === "solid") {
        assert.equal(t["--shell-fill"], d.bg, `${id}: solid panels are opaque`);
        assert.equal(t["--float-fill"], d.light ? mix(d.raised, "#ffffff", 0.3) : d.surface);
      } else {
        assert.match(t["--shell-fill"], /color-mix\(in srgb, #[0-9a-f]{6} \d+%, transparent\)/, `${id}/${style} fill`);
      }
      assert.equal(t["--shell-grain"] === "none", style !== "glass", `${id}/${style}: grain only on glass`);
      assert.equal(t["--center-scrim"], "transparent");
      assert.equal(t["--composer-fill"], "var(--color-surface-elevated)");
    }
  }
});

test("solid without a clear chat area keeps every colour token of the theme engine", () => {
  for (const id of THEMES) {
    const t = buildThemeTokens(theme(id), "#3b82f6", "solid", true, DEFAULT_SURFACE);
    const d = deriveTheme(theme(id));
    assert.equal(t["--color-text-muted"], d.muted);
    assert.equal(t["--color-success"], d.success);
  }
});

test("the transparency slider moves glass between the material maximum and the legibility floor", () => {
  for (const id of THEMES) {
    const d = deriveTheme(theme(id));
    const opaque = planSurfaces(d, "glass", true, { ...DEFAULT_SURFACE, transparency: 0, tone: flat(d.light ? "#202020" : "#303030") });
    const clear = planSurfaces(d, "glass", true, { ...DEFAULT_SURFACE, transparency: 100, tone: flat(d.light ? "#202020" : "#303030") });
    assert.equal(opaque.shellAlpha, 0.9, `${id}: 0 % = the glass maximum`);
    assert.ok(clear.shellAlpha < opaque.shellAlpha, `${id}: 100 % is clearer`);
    // Over the worst backdrop the floor wins: text stays at TEXT_TARGET.
    const worst = planSurfaces(d, "glass", true, { ...DEFAULT_SURFACE, transparency: 100, tone: WORST_TONE });
    const behind = d.light ? "#000000" : "#ffffff";
    assert.ok(contrastRatio(d.text, mix(behind, worst.tint, worst.shellAlpha)) >= TEXT_TARGET, `${id}: floor holds`);
    assert.ok(worst.shellAlpha > clear.shellAlpha, `${id}: a bright backdrop needs more fill than a dark one`);
  }
});

test("legibleAlpha is the smallest alpha that reaches the target", () => {
  const a = legibleAlpha("#ecedf1", "#111217", "#ffffff", 7);
  assert.ok(contrastRatio("#ecedf1", mix("#ffffff", "#111217", a)) >= 7);
  assert.ok(contrastRatio("#ecedf1", mix("#ffffff", "#111217", a - 0.01)) < 7);
  assert.equal(legibleAlpha("#ecedf1", "#111217", "#000000", 7), 0, "a black backdrop needs no tint for light text");
});

test("transparent chat area: no fill, a scrim that fades over dark backdrops and grows over bright ones", () => {
  for (const id of THEMES) {
    const d = deriveTheme(theme(id));
    const friendly = planSurfaces(d, "glass", true, { ...DEFAULT_SURFACE, transparentChat: true, tone: flat(d.bg) });
    const hostile = planSurfaces(d, "glass", true, { ...DEFAULT_SURFACE, transparentChat: true, tone: WORST_TONE });
    assert.equal(friendly.scrimAlpha, 0, `${id}: the theme's own colour behind needs no scrim`);
    // Dots on a calm ground: contrast may pass, but the detail still gets a scrim.
    const dotted: BackdropTone = d.light
      ? { peak: { light: d.bg, dark: "#c8c8c8" }, soft: { light: d.bg, dark: d.bg }, detail: { light: d.bg, dark: "#c8c8c8" } }
      : { peak: { light: "#3a3a3a", dark: d.bg }, soft: { light: d.bg, dark: d.bg }, detail: { light: "#3a3a3a", dark: d.bg } };
    assert.equal(legibleAlpha(d.text, d.light ? "#ffffff" : d.bg, d.light ? "#c8c8c8" : "#3a3a3a"), 0, `${id}: contrast alone passes`);
    const busy = planSurfaces(d, "glass", true, { ...DEFAULT_SURFACE, transparentChat: true, tone: dotted });
    assert.ok(busy.scrimAlpha >= 0.45, `${id}: busy backdrop gets the minimum scrim`);
    assert.ok(hostile.scrimAlpha > 0.5, `${id}: a hostile backdrop gets a real scrim`);
    const t = buildThemeTokens(theme(id), "#3b82f6", "glass", true, { ...DEFAULT_SURFACE, transparentChat: true, tone: WORST_TONE });
    assert.equal(t["--center-fill"], "transparent");
    assert.equal(t["--center-filter"], "none", `${id}: no blur under the conversation`);
    assert.notEqual(t["--composer-fill"], "var(--color-surface-elevated)", `${id}: the composer keeps its own surface`);
    assert.ok(alphaOf(t["--composer-fill"]) >= 0.62, `${id}: composer surface stays readable`);
  }
});

test("platform fallbacks", () => {
  assert.equal(effectiveUiStyle("glass", { backdropFilter: false, reducedTransparency: false }), "tinted");
  assert.equal(effectiveUiStyle("tinted", { backdropFilter: true, reducedTransparency: true }), "solid");
  assert.equal(effectiveUiStyle("line", { backdropFilter: true, reducedTransparency: false }), "line");
  assert.equal(effectiveUiStyle("line", { backdropFilter: true, reducedTransparency: true }), "line", "line keeps its outlines");
  // The shell geometry follows the painted style, not the saved one.
  assert.equal(isFloatShell("tinted", false, (s) => s), true);
  assert.equal(isFloatShell("glass", false, () => "solid"), false);
  assert.equal(isFloatShell("solid", true, () => "solid"), true);
});

test("stored surface values are clamped and validated", () => {
  assert.equal(parsePanelTransparency(-5), 0);
  assert.equal(parsePanelTransparency("250"), 100);
  assert.equal(parsePanelTransparency("lots"), DEFAULT_SURFACE.transparency);
  assert.equal(parsePanelBlur(1), PANEL_BLUR_MIN);
  assert.equal(parsePanelBlur(999), PANEL_BLUR_MAX);
  assert.equal(parsePanelBlur(null), DEFAULT_SURFACE.blur);
  assert.equal(parseTransparentChat(true), true);
  assert.equal(parseTransparentChat("yes"), false);
  assert.equal(parseTransparentChat(undefined), false);
});

test("Solid and Line keep the pre-244-E shell look: card shadow, no top highlight, the theme's border", () => {
  for (const id of THEMES) {
    for (const style of ["solid", "line"] as const) {
      for (const backdropActive of [false, true]) {
        const t = buildThemeTokens(theme(id), "#3b82f6", style, backdropActive, DEFAULT_SURFACE);
        const where = `${id}/${style}/${backdropActive ? "backdrop" : "plain"}`;
        assert.equal(t["--shell-shadow"], "var(--shadow-card)", `${where}: shell shadow`);
        assert.equal(t["--center-shadow"], "var(--shadow-card)", `${where}: center shadow`);
        assert.equal(t["--shell-highlight"], "inset 0 0 0 transparent", `${where}: no top highlight`);
        assert.equal(t["--center-highlight"], "inset 0 0 0 transparent", `${where}: no top highlight (center)`);
        assert.equal(t["--shell-border"], "var(--color-border)", `${where}: the style's own border`);
        assert.equal(t["--shell-grain"], "none");
      }
    }
    // The translucent styles are the only ones with the new material.
    for (const style of ["tinted", "glass"] as const) {
      const t = buildThemeTokens(theme(id), "#3b82f6", style, true, DEFAULT_SURFACE);
      assert.notEqual(t["--shell-shadow"], "var(--shadow-card)", `${id}/${style}: layered shadow`);
      assert.notEqual(t["--shell-highlight"], "inset 0 0 0 transparent", `${id}/${style}: top highlight`);
    }
  }
});

test("modal over a light theme: the floor accounts for the darkened backdrop behind the dialog", () => {
  const d = deriveTheme(theme("clay"));
  const tone = { peak: { light: "#000000", dark: "#000000" }, soft: { light: "#000000", dark: "#000000" } };
  const glass = planSurfaces(d, "glass", true, { ...DEFAULT_SURFACE, transparency: 100, tone });
  const layer = glass.layers.find((l) => l.name.includes("floating"));
  assert.ok(layer, "floating layer");
  assert.ok(contrastRatio(d.text, layer.color) >= 4.5, `floating surface ${contrastRatio(d.text, layer.color).toFixed(2)}`);
});
