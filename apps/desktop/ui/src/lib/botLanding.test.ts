import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  backFromChatOpensPanel,
  botChats,
  botDialogFromScreen,
  botLanding,
  chatActivity
} from "./botLanding.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");

const CHATS = [
  { id: "c1", bot_id: "atlas", created_at_ms: 10 },
  { id: "c2", bot_id: "atlas", created_at_ms: 30 },
  { id: "c3", bot_id: "nova", created_at_ms: 99 },
  { id: "c4", bot_id: "atlas", created_at_ms: 20 }
];

test("a bot opens its latest chat", () => {
  assert.deepEqual(botLanding("atlas", CHATS), { kind: "chat", chatId: "c2", botId: "atlas" });
  assert.deepEqual(botLanding("nova", CHATS), { kind: "chat", chatId: "c3", botId: "nova" });
});

test("a bot without chats opens the empty state, never a bot page", () => {
  assert.deepEqual(botLanding("milo", CHATS), { kind: "bot", botId: "milo" });
  assert.deepEqual(botLanding("atlas", []), { kind: "bot", botId: "atlas" });
});

test("botChats lists only that bot's chats, newest first, without mutating", () => {
  const before = CHATS.map((c) => c.id);
  assert.deepEqual(botChats(CHATS, "atlas").map((c) => c.id), ["c2", "c4", "c1"]);
  assert.deepEqual(CHATS.map((c) => c.id), before);
});

test("back from a chat opens the panel only when it is closed", () => {
  assert.equal(backFromChatOpensPanel(false), true);
  assert.equal(backFromChatOpensPanel(true), false);
});

test("chat activity: needs wins over running; finished runs are silent", () => {
  const runs = [
    { chat_id: "c1", state: "running" },
    { chat_id: "c2", state: "waiting_approval" },
    { chat_id: "c4", state: "succeeded" }
  ];
  assert.equal(chatActivity("c1", [], runs), "running");
  assert.equal(chatActivity("c2", [], runs), "running");
  assert.equal(chatActivity("c1", [{ chatId: "c1" }], runs), "needs");
  assert.equal(chatActivity("c4", [], runs), null);
  assert.equal(chatActivity("zz", [], runs), null);
});

test("mock review screens map to bot dialogs", () => {
  assert.equal(botDialogFromScreen("botform"), "edit");
  assert.equal(botDialogFromScreen("botprofile"), "profile");
  assert.equal(botDialogFromScreen("skills"), "skills");
  assert.equal(botDialogFromScreen("addskill"), "addSkill");
  assert.equal(botDialogFromScreen("bot"), null);
  assert.equal(botDialogFromScreen(null), null);
});

// Wiring: the shell must route through these functions, and the removed pages must stay gone.
test("App routes bots through botLanding and has no bot page or form route", () => {
  const app = read("../App.tsx");
  assert.match(app, /botLanding\(/);
  assert.match(app, /backFromChatOpensPanel\(/);
  assert.match(app, /botDialogFromScreen\(/);
  assert.doesNotMatch(app, /BotHomeView/);
  assert.doesNotMatch(app, /BotFormView/);
  assert.doesNotMatch(app, /kind: "botForm"/);
  // Edit and profile are dialogs.
  assert.match(app, /<BotEditModal/);
  assert.match(app, /<BotProfileModal/);
  assert.match(app, /<BotEmptyView/);
});

test("the right panel uses the tested chat and model helpers", () => {
  const panel = read("../components/BotPanel.tsx");
  assert.match(panel, /chatActivity\(/);
  assert.match(panel, /modelLine\(/);
  assert.doesNotMatch(panel, /split\("\/"\)\.pop\(\)/);
});
