import assert from "node:assert/strict";
import { test } from "node:test";
import { ApiError } from "../api/client.ts";
import { MockApiClient } from "../api/mock.ts";
import type { DoctorCheck } from "../api/types.ts";
import {
  INITIAL_DIAGNOSTICS,
  checkTone,
  diagnosticsReducer,
  repairButton,
  sortChecks,
  summarize
} from "./diagnostics.ts";

function check(overrides: Partial<DoctorCheck> = {}): DoctorCheck {
  return {
    id: "data_dir",
    title: "data dir",
    status: "ok",
    detail: "fine",
    hint: null,
    repair: null,
    ...overrides
  };
}

test("status renders as a tone; skipped is neutral, never an error", () => {
  assert.equal(checkTone("ok"), "ok");
  assert.equal(checkTone("warn"), "warn");
  assert.equal(checkTone("fail"), "fail");
  assert.equal(checkTone("skipped"), "neutral");
  const s = summarize([
    check({ status: "skipped" }),
    check({ status: "skipped" }),
    check({ status: "ok" })
  ]);
  assert.deepEqual(s, { fail: 0, warn: 0, ok: 1, neutral: 2, overall: "ok" });
  assert.equal(summarize([check({ status: "warn" }), check({ status: "fail" })]).overall, "fail");
});

test("problems sort first, otherwise the daemon order is kept", () => {
  const sorted = sortChecks([
    check({ id: "a", status: "ok" }),
    check({ id: "b", status: "skipped" }),
    check({ id: "c", status: "warn" }),
    check({ id: "d", status: "ok" }),
    check({ id: "e", status: "fail" })
  ]);
  assert.deepEqual(sorted.map((c) => c.id), ["e", "c", "a", "d", "b"]);
});

test("a repair button only appears for a fixable warning or failure", () => {
  const idle = { phase: "ready" as const, running: null };
  assert.equal(repairButton(check({ status: "fail" }), idle), null);
  assert.equal(repairButton(check({ status: "ok", repair: "create_data_dir" }), idle), null);
  assert.equal(repairButton(check({ status: "skipped", repair: "create_data_dir" }), idle), null);
  assert.deepEqual(repairButton(check({ status: "warn", repair: "create_data_dir" }), idle), {
    repair: "create_data_dir",
    disabled: false,
    busy: false
  });
});

test("repair buttons are disabled while a repair runs or checks reload", () => {
  const fixable = check({ status: "fail", repair: "reload_providers" });
  const other = check({ id: "data_dir", status: "warn", repair: "create_data_dir" });
  let state = diagnosticsReducer(INITIAL_DIAGNOSTICS, { type: "load_ok", checks: [fixable, other] });
  state = diagnosticsReducer(state, { type: "repair_start", repair: "reload_providers" });
  assert.deepEqual(repairButton(fixable, state), { repair: "reload_providers", disabled: true, busy: true });
  assert.deepEqual(repairButton(other, state), { repair: "create_data_dir", disabled: true, busy: false });
  // A second click while one runs is ignored.
  const again = diagnosticsReducer(state, { type: "repair_start", repair: "create_data_dir" });
  assert.equal(again.running, "reload_providers");
  state = diagnosticsReducer(state, {
    type: "repair_done",
    checkId: fixable.id,
    outcome: { ok: true, text: "1 provider(s) loaded" }
  });
  assert.equal(state.running, null);
  assert.equal(repairButton(other, state)?.disabled, false);
  assert.deepEqual(state.outcomes[fixable.id], { ok: true, text: "1 provider(s) loaded" });
  const loading = diagnosticsReducer(state, { type: "load_start" });
  assert.equal(repairButton(other, loading)?.disabled, true);
});

test("load errors are kept until the next successful load", () => {
  let state = diagnosticsReducer(INITIAL_DIAGNOSTICS, { type: "load_start" });
  assert.equal(state.phase, "loading");
  state = diagnosticsReducer(state, { type: "load_error", error: "offline" });
  assert.equal(state.phase, "error");
  assert.equal(state.error, "offline");
  state = diagnosticsReducer(state, { type: "load_ok", checks: [check()] });
  assert.equal(state.error, null);
  assert.equal(state.phase, "ready");
});

test("the client talks to the doctor routes; the mock fixes only allow-listed repairs", async () => {
  const client = new MockApiClient();
  const report = await client.doctor();
  assert.equal(report.ok, false);
  const fixable = report.checks.find((c) => c.repair === "reload_providers");
  assert.ok(fixable);
  const result = await client.doctorRepair("reload_providers");
  assert.equal(result.repair, "reload_providers");
  assert.deepEqual(client.repairs, ["reload_providers"]);
  const after = await client.doctor();
  assert.equal(after.checks.find((c) => c.id === fixable.id)?.status, "ok");
  await assert.rejects(
    client.doctorRepair("regenerate_api_token" as never),
    (e: unknown) => e instanceof ApiError && e.status === 404
  );
  const md = await client.doctorReport();
  assert.ok(md.startsWith("# Praxi diagnostics report"));
});
