import assert from "node:assert/strict";
import { test } from "node:test";
import { humanToolError } from "./toolError.ts";

test("the paused-observation refusal becomes a human sentence, other errors lose the transport prefix", () => {
  const t = (k: string) => `T:${k}`;
  assert.equal(
    humanToolError("Error: execution error: The user is controlling the screen right now (observation paused), so this read was not performed", t),
    "T:chat.tool.errors.observationPaused"
  );
  assert.equal(humanToolError("Error: execution error: file not found", t), "file not found");
});
