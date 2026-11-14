import assert from "node:assert/strict";
import { test } from "node:test";
import { botDetails, formatFiring, scheduleDetails } from "./approvalDetails.ts";

const card = {
  kind: "schedule",
  action: "create",
  name: "Standup",
  prompt: "Summarise my inbox.",
  schedule: "Every weekday at 10:30",
  timezone: "Europe/Istanbul",
  next_runs: [
    "2026-11-13T10:30:00+03:00",
    "2026-11-16T10:30:00+03:00",
    "2026-11-17T10:30:00+03:00"
  ],
  permission: { level: "ask_first", allow: [], ask: ["fs", "shell"], deny: ["money"] }
};

test("a schedule card is parsed with the prompt, timing, firings and permission", () => {
  const d = scheduleDetails(card);
  assert.ok(d);
  assert.equal(d.action, "create");
  assert.equal(d.prompt, "Summarise my inbox.");
  assert.equal(d.schedule, "Every weekday at 10:30");
  assert.equal(d.timezone, "Europe/Istanbul");
  assert.equal(d.nextRuns.length, 3);
  assert.equal(d.permission?.level, "ask_first");
  assert.deepEqual(d.permission?.deny, ["money"]);
});

test("an update carries old and new, pause carries name and timing", () => {
  const d = scheduleDetails({
    ...card,
    action: "update",
    prompt_chars: 19,
    previous: { name: "Old", prompt: "old p", schedule: "Every day at 09:00", timezone: "UTC" }
  });
  assert.ok(d);
  assert.equal(d.promptChars, 19);
  assert.equal(d.previous?.prompt, "old p");
  assert.equal(d.previous?.schedule, "Every day at 09:00");
  const p = scheduleDetails({
    kind: "schedule",
    action: "pause",
    name: "Nightly",
    schedule: "Every day at 09:00",
    prompt: ""
  });
  assert.equal(p?.name, "Nightly");
  assert.equal(p?.schedule, "Every day at 09:00");
  assert.equal(p?.previous, null);
});

test("anything else is not a schedule card", () => {
  assert.equal(scheduleDetails(undefined), null);
  assert.equal(scheduleDetails(null), null);
  assert.equal(scheduleDetails("x"), null);
  assert.equal(scheduleDetails({ kind: "other" }), null);
  assert.equal(scheduleDetails({ ...card, action: "explode" }), null);
});

test("missing or malformed fields degrade instead of throwing", () => {
  const d = scheduleDetails({ kind: "schedule", action: "resume", next_runs: [1, "a"] });
  assert.ok(d);
  assert.equal(d.prompt, "");
  assert.deepEqual(d.nextRuns, ["a"]);
  assert.equal(d.permission, null);
});

test("a firing keeps the wall-clock time of its own offset", () => {
  assert.match(formatFiring("2026-11-13T10:30:00+03:00", "en"), /10:30$/);
  assert.match(formatFiring("2026-11-13T09:00:00-05:00", "en"), /09:00$/);
  assert.equal(formatFiring("garbage"), "garbage");
});

const botCard = {
  kind: "bot",
  action: "update",
  bot_id: "b1",
  name: "Inbox",
  avatar: "data:image/png;base64,iVBORw0KGgo=",
  changes: [{ field: "bot.persona", from: "old", to: "new" }],
  profile: {
    bot: { name: "Inbox", persona: "Summarise.\nThen stop.", color: "#aabbcc" },
    models: { executor: "fake/echo", planner: "fake/big" },
    permissions: { preset: "approve-destructive" },
    tools: { allow: ["fs_read"], exclude: ["shell_run"] }
  }
};

test("a bot card carries the full persona, models, permissions, tools and diff", () => {
  const d = botDetails(botCard);
  assert.ok(d);
  assert.equal(d.action, "update");
  assert.equal(d.persona, "Summarise.\nThen stop.");
  assert.equal(d.executor, "fake/echo");
  assert.equal(d.planner, "fake/big");
  assert.equal(d.preset, "approve-destructive");
  assert.deepEqual(d.toolsAllow, ["fs_read"]);
  assert.deepEqual(d.toolsExclude, ["shell_run"]);
  assert.equal(d.color, "#aabbcc");
  assert.deepEqual(d.changes, [{ field: "bot.persona", from: "old", to: "new" }]);
  assert.match(d.profileJson, /Summarise/);
});

test("a bot card refuses unknown kinds, unknown actions and non-image avatars", () => {
  assert.equal(botDetails({ ...botCard, kind: "schedule" }), null);
  assert.equal(botDetails({ ...botCard, action: "promote" }), null);
  assert.equal(botDetails(null), null);
  assert.equal(botDetails({ ...botCard, avatar: "https://evil.example/x.png" })?.avatar, null);
  assert.equal(
    botDetails({ ...botCard, avatar: "data:text/html;base64,AAAA" })?.avatar,
    null
  );
  assert.equal(botDetails({ ...botCard, profile: { bot: { color: "red" } } })?.color, "");
  assert.equal(
    botDetails({
      ...botCard,
      profile: { permissions: { categories: { fs: {} } } }
    })?.preset,
    "custom"
  );
});
