import assert from "node:assert/strict";
import { test } from "node:test";
import {
  COMPUTER_FOOTER_KEY,
  computerFooter,
  computerPlaceholderKey,
  NO_SCREEN_CODES,
  computerPanel,
  isReadyIdle,
  noScreenCode,
  noScreenDetailKey,
  type ComputerPanelInput,
  type PanelText
} from "./computerLabel.ts";
import { readFileSync } from "node:fs";

test("no screen yet is only said after a confirmed empty list", () => {
  assert.equal(computerFooter(null, "ok"), "noScreen");
  assert.equal(computerFooter(null, "unknown"), "checking");
  assert.equal(computerFooter(null, "failed"), "queryFailed");
});

test("a known target keeps its own state whatever the query status", () => {
  const up = { available: true, canInput: true };
  assert.equal(computerFooter(up, "failed"), "interactive");
  assert.equal(computerFooter({ available: true, canInput: false }, "ok"), "viewOnly");
  assert.equal(computerFooter({ available: false, canInput: false }, "ok"), "unavailable");
});

test("placeholder text follows the query status", () => {
  assert.equal(computerPlaceholderKey("ok"), "liveView.noTargets");
  assert.equal(computerPlaceholderKey("unknown"), "liveView.checkingTargets");
  assert.equal(computerPlaceholderKey("failed"), "liveView.queryFailed");
  assert.equal(Object.keys(COMPUTER_FOOTER_KEY).length, 6);
});

test("the host's reason is a code, desktop first, never free text", () => {
  assert.equal(noScreenCode([]), null);
  assert.equal(noScreenCode([{ kind: "desktop", code: "" }]), null);
  assert.equal(
    noScreenCode([
      { kind: "browser", code: "start_failed" },
      { kind: "desktop", code: "wayland" }
    ]),
    "wayland"
  );
  assert.equal(noScreenDetailKey("wayland"), "liveView.noScreen.wayland");
  // A code from a newer daemon stays generic instead of showing raw text.
  assert.equal(noScreenDetailKey("/home/alice/x"), "liveView.noScreen.unknown");
});

test("an available screen nobody watches is ready, not watching live", () => {
  assert.equal(isReadyIdle("watch", false, true), true);
  assert.equal(isReadyIdle("watch", true, true), false);
  assert.equal(isReadyIdle("watch", false, false), false);
  assert.equal(isReadyIdle("active", false, true), false);
});

// ---- the rendered block: one model for pill + preview, in both locales ----

type Locale = Record<string, unknown>;
function load(name: string): Locale {
  return JSON.parse(
    readFileSync(new URL(`../i18n/locales/${name}.json`, import.meta.url), "utf8")
  ) as Locale;
}
function lookup(locale: Locale, key: string): string {
  const found = key
    .split(".")
    .reduce<unknown>((o, part) => (o as Locale | undefined)?.[part], locale);
  assert.equal(typeof found, "string", `missing locale key ${key}`);
  return found as string;
}
/** What the component prints: the model's key with its localized detail. */
function render(locale: Locale, text: PanelText): string {
  const detail = text.detailKey ? lookup(locale, text.detailKey) : "";
  return lookup(locale, text.key).replace("{{detail}}", detail);
}

const base: ComputerPanelInput = {
  hasTarget: false,
  available: false,
  reason: null,
  viewState: "watch",
  hasFrame: false,
  targetsStatus: "ok",
  noScreen: null
};

test("every no-screen code reads in the user's language, never an English backend sentence in Turkish", () => {
  const en = load("en");
  const tr = load("tr");
  for (const code of [...NO_SCREEN_CODES, "something_new"]) {
    const input = { ...base, noScreen: code };
    const panel = computerPanel(input);
    const turkish = render(tr, panel.pill);
    const english = render(en, panel.pill);
    assert.match(turkish, /^Ekran yok: /, code);
    assert.match(english, /^No screen: /, code);
    assert.notEqual(turkish.slice("Ekran yok: ".length), english.slice("No screen: ".length), code);
    assert.deepEqual(panel.pill, panel.preview, "pill and preview say the same");
    // The detail comes from the locale, so neither wording carries a path or user name.
    assert.doesNotMatch(turkish + english, /home|alice|C:/);
  }
});

test("an available screen with no first frame shows a loading preview, not the no-targets placeholder", () => {
  const input = { ...base, hasTarget: true, available: true };
  const panel = computerPanel(input);
  assert.equal(panel.pill.key, "liveView.panelSummary.ready");
  assert.equal(panel.preview.key, "liveView.loadingPreview");
  for (const name of ["en", "tr"]) {
    const locale = load(name);
    assert.notEqual(render(locale, panel.preview), render(locale, { key: "liveView.noTargets" }));
  }
  // With a frame the block shows the image; the pill is the live one.
  assert.equal(computerPanel({ ...input, hasFrame: true }).pill.key, "liveView.panelSummary.watch");
});

test("pill and preview follow the query status when no screen is known", () => {
  assert.equal(computerPanel({ ...base, targetsStatus: "unknown" }).pill.key, "liveView.states.checking");
  assert.equal(computerPanel({ ...base, targetsStatus: "unknown" }).preview.key, "liveView.checkingTargets");
  assert.equal(computerPanel({ ...base, targetsStatus: "failed", noScreen: "no_display" }).pill.key, "liveView.states.queryFailed");
  assert.equal(computerPanel({ ...base, targetsStatus: "failed" }).preview.key, "liveView.queryFailed");
  // Only a confirmed empty list may say "no screen" (header and preview alike).
  assert.equal(computerPanel(base).pill.key, "liveView.panelSummary.empty");
  assert.equal(computerPanel(base).preview.key, "liveView.noTargets");
});

test("an unavailable known target says why, in both locales", () => {
  const panel = computerPanel({
    ...base,
    hasTarget: true,
    available: false,
    reason: "browser_idle"
  });
  assert.equal(panel.pill.key, "liveView.panelSummary.unavailable");
  assert.equal(panel.preview.key, "liveView.reasons.browser_idle");
  for (const name of ["en", "tr"]) {
    assert.ok(render(load(name), panel.preview).length > 0);
  }
});
