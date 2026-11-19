import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { sessionRequest } from "./liveViewRequest.ts";

test("the open viewer asks for the host's full 1920x1080 cap at quality 70", () => {
  const watch = sessionRequest({ viewerOpen: true, owning: false });
  assert.deepEqual(watch, { maxWidth: 1920, maxHeight: 1080, quality: 70 });
  const drive = sessionRequest({ viewerOpen: true, owning: true });
  assert.deepEqual(drive, watch);
});

test("the card asks for a smaller picture than the viewer", () => {
  const card = sessionRequest({ viewerOpen: false, owning: false });
  assert.ok(card.maxWidth <= 640 && card.maxHeight <= 400);
});

test("the stream picture is drawn without CSS filters or pixelation", () => {
  const css = readFileSync(new URL("../views/LiveViewer.module.css", import.meta.url), "utf8");
  const rule = css.slice(css.indexOf(".stageImg"), css.indexOf("}", css.indexOf(".stageImg")));
  assert.doesNotMatch(rule, /filter|blur|pixelated|transform/);
  assert.match(rule, /image-rendering: auto/);
});
