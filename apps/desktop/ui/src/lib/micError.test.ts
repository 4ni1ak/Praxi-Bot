import assert from "node:assert/strict";
import { test } from "node:test";
import { micErrorKey } from "./micError.ts";

test("a missing or busy microphone is told apart from a denied permission", () => {
  assert.equal(micErrorKey({ name: "NotAllowedError" }), "voice.micDenied");
  assert.equal(micErrorKey({ name: "NotFoundError" }), "voice.micMissing");
  assert.equal(micErrorKey({ name: "NotReadableError" }), "voice.micBusy");
  assert.equal(micErrorKey(new Error("x")), "voice.micDenied");
  assert.equal(micErrorKey(undefined), "voice.micDenied");
});
