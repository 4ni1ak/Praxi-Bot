import assert from "node:assert/strict";
import { test } from "node:test";
import { isProcessingRunState } from "./runState.ts";

test("queued runs are processing, the tool-item word pending and paused are not", () => {
  for (const s of ["queued", "running", "waiting_approval"]) assert.equal(isProcessingRunState(s), true);
  for (const s of ["pending", "paused", "succeeded", "failed", "cancelled"]) assert.equal(isProcessingRunState(s), false);
});
