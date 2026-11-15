import assert from "node:assert/strict";
import { test } from "node:test";
import { overrideOptionsFor, stopTransport } from "./stopHost.ts";

// The host wiring itself is exercised end to end in stopRuntime.test.ts.

test("stopTransport passes the body and the attempt signal through to the client", async () => {
  const seen: unknown[] = [];
  const transport = stopTransport({
    stopRuns: async (body, signal) => {
      seen.push(body, signal);
      return { stop_id: body.stop_id, first_seen: true, results: [] };
    }
  });
  const signal = new AbortController().signal;
  await transport.stopRuns({ stop_id: "x", scope: "listed", run_ids: [] }, signal);
  assert.deepEqual(seen, [{ stop_id: "x", scope: "listed", run_ids: [] }, signal]);
});

test("the viewer's Stop always overrides a takeover; every other origin only when controlled", () => {
  assert.deepEqual(overrideOptionsFor({ origin: "viewer" }), { onlyIfControlled: false });
  for (const origin of ["hotkey", "banner", "native", "chat"] as const) {
    assert.deepEqual(overrideOptionsFor({ origin }), { onlyIfControlled: true });
  }
});
