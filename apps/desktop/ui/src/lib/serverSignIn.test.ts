import assert from "node:assert/strict";
import { test } from "node:test";
import { apiKeyProvider, signInPhase } from "./serverSignIn.ts";

test("the sign-in phase follows the server's state and unknown states are idle", () => {
  for (const s of ["starting", "needs_browser", "needs_code", "verifying", "done", "failed", "timed_out", "cancelled"]) {
    assert.equal(signInPhase({ state: s }), s);
  }
  assert.equal(signInPhase(null), "idle");
  assert.equal(signInPhase({ state: "something-new" }), "idle");
});

test("only engines with a matching model provider offer the API-key shortcut", () => {
  assert.equal(apiKeyProvider("claude"), "anthropic");
  assert.equal(apiKeyProvider("codex"), "openai");
  for (const id of ["opencode", "cursor", "agy", "gemini", ""]) assert.equal(apiKeyProvider(id), null);
});
