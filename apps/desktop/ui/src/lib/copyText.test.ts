import assert from "node:assert/strict";
import { test } from "node:test";
import { copyText } from "./copyText.ts";

test("copyText writes the text, and reports false for empty text or a refused clipboard", async () => {
  let written = "";
  assert.equal(await copyText("hello", { writeText: async (s) => void (written = s) }), true);
  assert.equal(written, "hello");
  assert.equal(await copyText("  ", { writeText: async () => undefined }), false);
  assert.equal(
    await copyText("x", {
      writeText: async () => {
        throw new Error("denied");
      }
    }),
    false
  );
});
