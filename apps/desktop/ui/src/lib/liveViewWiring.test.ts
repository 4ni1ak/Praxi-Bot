import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

/**
 * Static guards for wiring that lives in React components and hooks, which the
 * DOM-less test runner cannot mount. They fail if the review-56 fixes are
 * undone in the source; behaviour itself is covered by the pure tests next to
 * them (liveViewStage / liveViewControl / controlBridge).
 */
const read = (rel: string) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");

const viewer = read("../views/LiveViewer.tsx");
const context = read("../state/LiveViewContext.tsx");
const app = read("../App.tsx");

test("LiveViewer wires key forwarding to one element and guards the local toolbar", () => {
  assert.equal(
    (viewer.match(/onKeyDown=/g) ?? []).length,
    0,
    "no inline key handlers: they come from the stage controller"
  );
  assert.match(viewer, /\{\.\.\.stageController\.ime\}/);
  assert.match(viewer, /\{\.\.\.stageController\.stage\}/);
  assert.match(viewer, /\{\.\.\.stageController\.toolbar\}/);
  // the stage div must not receive the textarea's key handlers
  const stageSpread = viewer.indexOf("{...stageController.stage}");
  const imeSpread = viewer.indexOf("{...stageController.ime}");
  assert.ok(stageSpread > 0 && imeSpread > 0 && stageSpread !== imeSpread);
});

test("LiveViewer locks the target selector while control is held", () => {
  assert.match(viewer, /disabled=\{controlBusy\}/);
});

test("the control bridge registration keeps its identity across control-state refreshes", () => {
  assert.match(context, /createBridgeRegistry\(/);
  assert.match(
    context,
    /registerBridge: \(bridge: ControlBridge \| null\) => void =\s*registry\.register/
  );
  assert.doesNotMatch(context, /bridgeRef/, "no per-render bridge ref");
  assert.doesNotMatch(
    context,
    /registerBridge = useCallback/,
    "registerBridge must not depend on controlState"
  );
});

test("every global Stop call site uses the takeover-only override", () => {
  const calls = app.match(/overrideStop\([^)]*\)/g) ?? [];
  assert.ok(calls.length >= 2, "hotkey and banner button");
  for (const c of calls) {
    assert.match(c, /onlyIfControlled: true/, c);
  }
  assert.match(app, /!event\.repeat/, "key repeat does not re-fire the override");
});

test("the viewer layer sits below dialogs and toasts so approvals stay answerable", () => {
  const z = (css: string, sel: string) => {
    const m = new RegExp(`${sel}\\s*\\{[^}]*?z-index:\\s*(\\d+)`).exec(css);
    assert.ok(m, `z-index of ${sel}`);
    return Number(m[1]);
  };
  const viewerZ = z(read("../views/LiveViewer.module.css"), "\\.backdrop");
  assert.ok(viewerZ < z(read("../components/Modal.module.css"), "\\.backdrop"));
  assert.ok(viewerZ < z(read("../components/Toast.module.css"), "\\.region"));
});

// ---- second review: which element carries which handler, and who calls what ----

const host = read("../state/ControlBridgeHost.tsx");
const modal = read("../components/Modal.tsx");
const palette = read("../components/CommandPalette.tsx");

/** The opening tag (`<div ... >`) that contains `needle`. */
function tagAround(source: string, needle: string): string {
  const at = source.indexOf(needle);
  assert.ok(at > 0, `${needle} present`);
  const open = Math.max(
    source.lastIndexOf("<div", at),
    source.lastIndexOf("<textarea", at),
    source.lastIndexOf("<Button", at)
  );
  return source.slice(open, source.indexOf(">", at) + 1);
}

test("each handler spread sits on the right element", () => {
  const stage = tagAround(viewer, "{...stageController.stage}");
  assert.match(stage, /^<div/);
  assert.match(stage, /ref=\{stageRef\}/);
  assert.doesNotMatch(stage, /stageController\.(ime|toolbar)/);

  const ime = tagAround(viewer, "{...stageController.ime}");
  assert.match(ime, /^<textarea/);
  assert.doesNotMatch(ime, /stageController\.(stage|toolbar)/);

  const toolbar = tagAround(viewer, "{...stageController.toolbar}");
  assert.match(toolbar, /^<div/);
  assert.match(toolbar, /focusOverlayBar/);
  assert.doesNotMatch(toolbar, /stageController\.(stage|ime)/);
});

test("ControlBridgeHost registers its bridge through the stable registerBridge", () => {
  assert.match(host, /registerBridge\(bridge\)/);
  assert.match(host, /registerBridge\(null\)/);
  const deps = /\}, \[([^\]]*)\]\);\s*return null;/.exec(host);
  assert.ok(deps, "effect dependency list");
  assert.match(deps[1], /\bregisterBridge\b/);
  assert.doesNotMatch(deps[1], /controlState/);
  assert.match(host, /onSnapshot: ingestControlSnapshot/);
  assert.equal((host.match(/new ControlBridge\(/g) ?? []).length, 1);
});

test("the context calls the tested helpers (release, close, reconnect)", () => {
  assert.match(context, /await runRelease\(/);
  assert.match(context, /planViewerClose\(stateRef\.current\)/);
  assert.match(context, /runReconnectLoop\(/);
  assert.match(context, /applyStreamDisconnected\(prev\)/);
  assert.match(context, /applySessionCreated\(prev, selectedTarget, session\)/);
  // dismissing never goes through performRelease
  const dismiss = /plan === "dismiss"\) \{[^}]*\}/.exec(context);
  assert.ok(dismiss);
  assert.doesNotMatch(dismiss[0], /performRelease/);
  assert.match(dismiss[0], /setViewerOpen\(false\)/);
  // the old per-attempt deleteViewSession on every drop is gone
  assert.equal(
    (context.match(/deleteViewSession\(/g) ?? []).length,
    4,
    "no-session cleanup, pagehide, effect cleanup, loop delete link"
  );
});

test("the viewer is not aria-modal, never disables its only dismiss, and defers keys to higher layers", () => {
  assert.doesNotMatch(viewer, /aria-modal/);
  const back = tagAround(viewer, 'data-testid="live-viewer-close"');
  assert.doesNotMatch(back, /disabled/);
  assert.match(viewer, /viewerKeyAction\(/);
  assert.doesNotMatch(viewer, /inert/);
});

test("Modal uses the ref-counted inert tracker and the palette is a layer", () => {
  assert.match(modal, /appInert\.acquire\(seq\)/);
  assert.match(modal, /appInert\.release\(seq\)/);
  assert.doesNotMatch(modal, /querySelectorAll\('\[aria-modal/);
  assert.match(palette, /layerStack\.add\(seq\)/);
  assert.match(palette, /e\.stopPropagation\(\)/);
});
