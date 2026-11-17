import assert from "node:assert/strict";
import { test } from "node:test";
import type { CopyPlan, CopyReport } from "../api/types.ts";
import { buildCopyItems, defaultChecked, resultKey, summarizeReport, toRequest } from "./copySetup.ts";

const plan: CopyPlan = {
  providers: [
    { id: "gemini", has_key: true, model: "m", custom: false },
    { id: "ollama", has_key: false, model: null, custom: false }
  ],
  bots: [
    { id: "b1", name: "Atlas", model: "gemini/m", blocked: null, schedules: 2, chats: 1, messages: 14 },
    { id: "b2", name: "Coder", model: "cli:claude/x", blocked: "cli_engine", schedules: 5, chats: 3, messages: 40 }
  ],
  default_provider: "gemini",
  default_model: "m",
  host_accepts: true,
  host_code: null,
  host_command: null
};

test("the dialog lists providers, bots and the default; blocked bots are not sent", () => {
  const items = buildCopyItems(plan);
  assert.deepEqual(
    items.map((i) => i.key),
    [
      "provider:gemini",
      "provider:ollama",
      "bot:b1",
      "bot:b2",
      "schedules:all",
      "chats:all",
      "memory:all",
      "default:default"
    ]
  );
  const checked = defaultChecked(items);
  assert.equal(checked.has("bot:b2"), false);
  checked.delete("provider:ollama");
  const req = toRequest(items, checked);
  assert.deepEqual(req.providers, ["gemini"]);
  assert.deepEqual(req.bots, ["b1"]);
  assert.equal(req.default_model, true);
  // Schedules and chats are counted for the bots that can move; memory is listed, never sent.
  const byKey = Object.fromEntries(items.map((i) => [i.key, i]));
  assert.equal(byKey["schedules:all"].count, 2);
  assert.equal(byKey["chats:all"].messages, 14);
  assert.equal(byKey["memory:all"].blocked, "no_export");
  assert.equal(req.schedules, true);
  assert.equal(req.chats, true);
  checked.delete("schedules:all");
  assert.equal(toRequest(items, checked).schedules, false);
  // Only ids: the request never carries a key.
  assert.equal(JSON.stringify(req).includes("sk-"), false);
});

test("the report is summed per item and a home failure is surfaced", () => {
  const report: CopyReport = {
    items: [
      { kind: "provider", id: "gemini", name: "gemini", status: "done", code: null },
      { kind: "bot", id: "b1", name: "Atlas", status: "skipped", code: "exists" },
      { kind: "bot", id: "b3", name: "X", status: "failed", code: "host_admin_required" },
      { kind: "home", id: "home", name: "home", status: "failed", code: "home_not_recorded" }
    ]
  };
  assert.deepEqual(summarizeReport(report), { done: 1, skipped: 1, failed: 1, homeFailed: true });
  assert.equal(resultKey(report.items[1]), "exists");
  assert.equal(resultKey(report.items[0]), "done");
});
