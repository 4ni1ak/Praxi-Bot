import assert from "node:assert/strict";
import { test } from "node:test";
import type { ProvidersDto } from "../api/types.ts";
import {
  autoModelFor,
  canContinue,
  classifyConnectError,
  firstRunReducer,
  INITIAL_FIRST_RUN,
  type FirstRunAction,
  type FirstRunState
} from "./firstRun.ts";

const run = (actions: FirstRunAction[], from: FirstRunState = INITIAL_FIRST_RUN) =>
  actions.reduce(firstRunReducer, from);

test("a signed-in CLI skips the key step and reaches the freedom screen", () => {
  const picked = run([{ type: "pickCli", id: "claude", model: "cli:claude/default" }]);
  assert.equal(picked.phase, "connected");
  assert.equal(canContinue(picked), true);
  const next = run([{ type: "next" }], picked);
  assert.equal(next.step, "freedom");
  assert.equal(next.level, "balanced");
});

test("the API path needs a validated key before it can continue", () => {
  const key = run([{ type: "pickApi", group: "gemini" }]);
  assert.equal(key.phase, "key");
  assert.equal(run([{ type: "next" }], key).step, "brain");

  const failed = run([{ type: "connecting" }, { type: "failed", error: "badKey" }], key);
  assert.equal(failed.phase, "key");
  assert.equal(failed.error, "badKey");
  assert.equal(canContinue(failed), false);

  const ok = run([{ type: "connecting" }, { type: "connected", model: "gemini/gemini-3.5-flash-lite" }], failed);
  assert.equal(ok.error, null);
  assert.equal(run([{ type: "next" }, { type: "next" }], ok).step, "bot");
  // Ollama has no key: it goes straight to the check.
  assert.equal(run([{ type: "pickApi", group: "ollama" }]).phase, "connecting");
});

test("skip ends the flow from any screen; the brain can be changed after connecting", () => {
  const mid = run([{ type: "pickCli", id: "claude", model: "cli:claude/default" }, { type: "next" }]);
  assert.equal(run([{ type: "skip" }], mid).outcome, "skipped");
  assert.equal(run([{ type: "skip" }]).outcome, "skipped");
  const reset = run([{ type: "changeBrain" }], mid);
  assert.deepEqual([reset.phase, reset.choice, reset.model], ["choose", null, ""]);
});

test("the automatic model follows the picker's recommendation rules; errors are classified", () => {
  const detail = (id: string) => ({ id, display_name: id, kind: "chat", recommended: false, recommended_rank: null, deprecated: false, note: null });
  const ids = ["gemini-2.5-pro", "gemini-3.5-flash-lite", "gemini-3.5-flash"];
  const dto = {
    active: ["gemini"],
    default_provider: "gemini",
    catalog: [],
    configured: [{
      id: "gemini", active: true, custom: false, default_model: ids[0], enabled: true,
      has_key: true, is_default: true, models: ids, model_details: ids.map(detail)
    }]
  } as unknown as ProvidersDto;
  assert.deepEqual(autoModelFor(dto, "gemini"), { ref: "gemini/gemini-3.5-flash-lite", recommended: true });
  assert.equal(autoModelFor(dto, "anthropic"), null);

  assert.equal(classifyConnectError("HTTP 401 Unauthorized", "gemini"), "badKey");
  assert.equal(classifyConnectError("connection refused", "ollama"), "ollamaDown");
  assert.equal(classifyConnectError("dns error: failed to resolve host", "anthropic"), "offline");
  assert.equal(classifyConnectError("something odd", "anthropic"), "unknown");
});

test("the first bot of a fresh home is Praxi, the white ghost", async () => {
  const { FIRST_RUN_BOT_COLOR, FIRST_RUN_BOT_SHAPE } = await import("./firstRun.ts");
  assert.equal(FIRST_RUN_BOT_SHAPE, "ghost");
  assert.equal(FIRST_RUN_BOT_COLOR, "#f4f4f5");
});
