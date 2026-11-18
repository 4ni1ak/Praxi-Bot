import assert from "node:assert/strict";
import { test } from "node:test";
import { appendDictation, pickRecordingMime } from "./dictation.ts";

test("dictated text joins the draft with one space and ignores blanks", () => {
  assert.equal(appendDictation("", " hello "), "hello");
  assert.equal(appendDictation("Dear Ana,", "thanks"), "Dear Ana, thanks");
  assert.equal(appendDictation("line\n", "next"), "line\nnext");
  assert.equal(appendDictation("keep", "  "), "keep");
});

test("recording format falls back through the supported list", () => {
  assert.equal(pickRecordingMime((m) => m === "audio/mp4"), "audio/mp4");
  assert.equal(pickRecordingMime(() => false), undefined);
});
