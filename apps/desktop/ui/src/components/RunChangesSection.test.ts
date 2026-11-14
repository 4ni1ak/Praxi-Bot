import assert from "node:assert/strict";
import { test } from "node:test";
import { createElement, act } from "react";
import { createRoot } from "react-dom/client";
import { JSDOM } from "jsdom";
import { createServer } from "vite";
import { readFileSync } from "node:fs";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import type { ApiClient } from "../api/client.ts";
import type { RestorePreview, RunChanges } from "../api/types.ts";

// Exercise the real component, portal, checkbox and API calls. Vite handles
// TSX and CSS modules exactly as in the app; jsdom supplies only the browser.
test("changes opens lazily, paginates, resets consent on 409, and exposes undo safety", async () => {
  const dom = new JSDOM('<html><body><div id="root"></div></body></html>', { url: "http://localhost" });
  const saved = new Map<string, PropertyDescriptor | undefined>();
  const globals: Record<string, unknown> = {
    window: dom.window, document: dom.window.document,
    HTMLElement: dom.window.HTMLElement, navigator: dom.window.navigator,
    IS_REACT_ACT_ENVIRONMENT: true
  };
  for (const [key, value] of Object.entries(globals)) {
    saved.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
    Object.defineProperty(globalThis, key, { configurable: true, value });
  }
  await i18n.use(initReactI18next).init({ lng: "en", resources: { en: { translation: JSON.parse(readFileSync(new URL("../i18n/locales/en.json", import.meta.url), "utf8")) } } });
  const server = await createServer({ optimizeDeps: { noDiscovery: true }, configFile: false, server: { middlewareMode: true }, appType: "custom" });
  const root = createRoot(document.getElementById("root")!);
  try {
    const { RunChangesSection } = await server.ssrLoadModule("/src/components/RunChangesSection.tsx");
    const { ApiError } = await server.ssrLoadModule("/src/api/client.ts");
    let pages = 0;
    let previews = 0;
    const restores: Array<{ id: string; force: boolean }> = [];
    const clean: RestorePreview = {
      run_id: "run", checkpoint_id: "cp-1", entries: [{ path: "/work/a.txt", accessible: true, changed_by_run: null, current_matches_after: true, action: "restore", via_link_of: null }],
      modified_since: [], covered: ["fs_edit"], not_covered: ["shell"]
    };
    const data: RunChanges = {
      run_id: "run", next_offset: 50, covered: ["fs_edit"], not_covered: ["shell"], changes: [{
        path: "/work/a.txt", accessible: true, changed_by_run: null, checkpoint_id: "cp-1", kind: "modified", op: "fs_edit", existed: true,
        before_size: 1, after_size: 1, current_matches_after: true, is_dir: false, is_link: false, text: null, diff_omitted_reason: null, moved_from: null, via_link_of: null
      }]
    };
    const client = {
      getRunChanges: async (_id: string, offset = 0) => { pages++; return offset ? { ...data, changes: [], next_offset: null } : data; },
      previewRestore: async (_id: string, cp: string) => {
        previews++;
        return previews === 1 || cp === "cp-safety" ? { ...clean, checkpoint_id: cp } : { ...clean, entries: [{ ...clean.entries[0], current_matches_after: false }], modified_since: ["/work/a.txt"] };
      },
      restoreRunChanges: async (_id: string, id: string, force: boolean) => {
        restores.push({ id, force });
        if (restores.length === 1) throw new ApiError(409, "modified_since", "race");
        return { run_id: "run", checkpoint_id: id, restored: ["/work/a.txt"], safety_checkpoint_ids: ["cp-safety"], warnings: [], partial: false, error: null, covered: [], not_covered: [] };
      }
    } as unknown as ApiClient;
    await act(async () => { root.render(createElement(RunChangesSection, { runId: "run", client, restoreSignal: 0 })); });
    assert.equal(pages, 0, "collapsed runs do not fetch on chat load");
    const element = <T extends Element>(id: string) => document.querySelector<T>(`[data-testid="${id}"]`)!;
    const click = async (id: string) => { await act(async () => { element<HTMLButtonElement>(id).click(); }); };
    await click("run-changes-toggle");
    assert.equal(pages, 1);
    await click("changes-load-more");
    assert.equal(pages, 2);
    await click("undo-file-button");
    assert.equal(previews, 1);
    assert.equal(element<HTMLButtonElement>("confirm-restore-button").disabled, false);
    assert.equal(document.querySelector('[data-testid="overwrite-newer-checkbox"]'), null);
    await click("confirm-restore-button");
    assert.equal(previews, 2, "409 reruns preview");
    assert.equal(element<HTMLButtonElement>("confirm-restore-button").disabled, true);
    assert.equal(element<HTMLInputElement>("overwrite-newer-checkbox").checked, false);
    await click("confirm-restore-button");
    assert.equal(restores.length, 1, "no retry until explicit consent");
    await click("overwrite-newer-checkbox");
    await click("confirm-restore-button");
    assert.deepEqual(restores, [{ id: "cp-1", force: false }, { id: "cp-1", force: true }]);
    // With no translation provider the id is still passed in the visible action;
    // the real locales interpolate it in the checkpoint label.
    assert.ok(element("safety-checkpoint").textContent?.includes("cp-safety"));
    await click("undo-undo-button");
    assert.equal(previews, 3);
    await click("confirm-restore-button");
    assert.deepEqual(restores[2], { id: "cp-safety", force: false });
  } finally {
    await act(async () => { root.unmount(); });
    await server.close();
    dom.window.close();
    for (const [key, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
