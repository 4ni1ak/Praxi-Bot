import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { MockApiClient } from "../api/mock.ts";
import type { Chat } from "../api/types.ts";
import { botChats, botLanding } from "./botLanding.ts";
import {
  applyChatDelete,
  applyChatPatch,
  CHAT_TITLE_MAX_LEN,
  chatHasActiveRun,
  mergeChatRefresh,
  nextRouteAfterChatDelete,
  orderChats,
  partitionChats,
  rollbackChatDelete,
  rollbackChatPatch,
  validateChatTitle
} from "./chatManage.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");

function makeChat(id: string, createdAtMs: number, extra: Partial<Chat> = {}): Chat {
  return {
    id,
    chat_id: id,
    bot_id: "bot-1",
    title: `Chat ${id}`,
    created_at_ms: createdAtMs,
    pinned: false,
    archived: false,
    ...extra
  };
}

test("rename validation trims whitespace, rejects empty/oversized titles, and optimistic patch rolls back cleanly", () => {
  assert.deepEqual(validateChatTitle("  Weekly notes  "), { ok: true, title: "Weekly notes" });
  assert.deepEqual(validateChatTitle("   "), { ok: false, reason: "empty" });
  assert.deepEqual(validateChatTitle("a".repeat(CHAT_TITLE_MAX_LEN)), {
    ok: true,
    title: "a".repeat(CHAT_TITLE_MAX_LEN)
  });
  assert.deepEqual(validateChatTitle("a".repeat(CHAT_TITLE_MAX_LEN + 1)), {
    ok: false,
    reason: "too_long"
  });

  const before = [makeChat("c1", 10, { title: "Old title" }), makeChat("c2", 20)];
  const { next, previous } = applyChatPatch(before, "c1", { title: "New title" });
  assert.equal(next[0]?.title, "New title");
  assert.equal(before[0]?.title, "Old title");
  assert.deepEqual(rollbackChatPatch(next, previous, { title: "New title" }), before);
});

test("pin ordering floats pinned chats above newer unpinned ones and archive splits them out", () => {
  const list = [
    makeChat("old-pinned", 10, { pinned: true }),
    makeChat("newest", 40),
    makeChat("mid-pinned", 25, { pinned: true }),
    makeChat("archived-newest", 50, { archived: true }),
    makeChat("archived-pinned", 5, { archived: true, pinned: true })
  ];

  assert.deepEqual(
    orderChats(list).map((c) => c.id),
    ["mid-pinned", "old-pinned", "archived-pinned", "archived-newest", "newest"]
  );
  assert.deepEqual(
    botChats(list, "bot-1").map((c) => c.id),
    ["mid-pinned", "old-pinned", "archived-pinned", "archived-newest", "newest"]
  );

  const { active, archived } = partitionChats(list);
  assert.deepEqual(
    active.map((c) => c.id),
    ["mid-pinned", "old-pinned", "newest"]
  );
  assert.deepEqual(
    archived.map((c) => c.id),
    ["archived-pinned", "archived-newest"]
  );

  // Bot landing skips archived chats and picks the top pinned active chat.
  assert.deepEqual(botLanding("bot-1", list), {
    kind: "chat",
    chatId: "mid-pinned",
    botId: "bot-1"
  });
  assert.deepEqual(
    botLanding("bot-1", [makeChat("only-archived", 10, { archived: true })]),
    { kind: "bot", botId: "bot-1" }
  );
});

test("delete guard detects active runs, optimistic delete rolls back at original index, and route falls back cleanly", () => {
  const runs = [
    { chat_id: "c1", state: "running" },
    { chat_id: "c2", state: "queued" },
    { chat_id: "c3", state: "waiting_approval" },
    { chat_id: "c4", state: "succeeded" },
    { chat_id: "c5", state: "failed" },
    { chat_id: "c6", state: "cancelled" }
  ];
  assert.equal(chatHasActiveRun("c1", runs), true);
  assert.equal(chatHasActiveRun("c2", runs), true);
  assert.equal(chatHasActiveRun("c3", runs), true);
  assert.equal(chatHasActiveRun("c4", runs), false);
  assert.equal(chatHasActiveRun("c5", runs), false);
  assert.equal(chatHasActiveRun("c6", runs), false);

  const list = [makeChat("c1", 10), makeChat("c2", 20), makeChat("c3", 30)];
  const { next, removed, index } = applyChatDelete(list, "c2");
  assert.deepEqual(
    next.map((c) => c.id),
    ["c1", "c3"]
  );
  assert.deepEqual(rollbackChatDelete(next, removed, index), list);

  assert.deepEqual(
    nextRouteAfterChatDelete("c2", "bot-1", next, { kind: "chat", chatId: "c2" }),
    { kind: "chat", chatId: "c3", botId: "bot-1" }
  );
  assert.equal(
    nextRouteAfterChatDelete("c2", "bot-1", next, { kind: "chat", chatId: "c1" }),
    null
  );
  assert.deepEqual(
    nextRouteAfterChatDelete("c1", "bot-1", [], { kind: "chat", chatId: "c1" }),
    { kind: "bot", botId: "bot-1" }
  );
});

test("MockApiClient serves PATCH and DELETE /chats with validation", async () => {
  const client = new MockApiClient();
  const created = await client.createChat("11111111-1111-4111-8111-111111111111", "Draft");
  const updated = await client.updateChat(created.id, {
    title: "  Renamed  ",
    pinned: true,
    archived: true
  });
  assert.equal(updated.title, "Renamed");
  assert.equal(updated.pinned, true);
  assert.equal(updated.archived, true);

  await assert.rejects(() => client.updateChat(created.id, { title: "   " }));
  await client.deleteChat(created.id);
  const all = await client.listChats();
  assert.equal(all.some((c) => c.id === created.id), false);
});

test("only the bot panel mounts ChatList (the left rail lists bots only), and deletion is gated behind ConfirmDialog", () => {
  const app = read("../App.tsx");
  const panel = read("../components/BotPanel.tsx");
  const list = read("../components/ChatList.tsx");
  assert.doesNotMatch(app, /<ChatList/);
  assert.doesNotMatch(app, /rail-chats|railChats/);
  assert.match(panel, /<ChatList[\s\S]*variant="panel"/);
  assert.match(list, /<ConfirmDialog[\s\S]*confirmDisabled=\{deleteActiveRun\}/);
  assert.match(list, /chatHasActiveRun\(/);
  assert.match(list, /setDeleteTarget\(menuChat\)/);
});

test("a refresh skips chats being deleted and keeps optimistic patches", () => {
  const current = [makeChat("c1", 10, { pinned: true }), makeChat("c2", 20)];
  const fresh = [makeChat("c1", 10), makeChat("c2", 20), makeChat("c3", 30)];
  const merged = mergeChatRefresh(fresh, current, new Set(["c2"]), new Set(["c1"]));
  assert.deepEqual(
    merged.map((c) => c.id),
    ["c1", "c3"]
  );
  assert.equal(merged[0]?.pinned, true);
});

test("rolling back a failed patch restores only the patched fields", () => {
  const previous = makeChat("c1", 10, { title: "Old", pinned: false });
  // Pin failed, but the title was renamed concurrently.
  const current = [makeChat("c1", 10, { title: "New", pinned: true })];
  const [c] = rollbackChatPatch(current, previous, { pinned: true });
  assert.equal(c?.pinned, false);
  assert.equal(c?.title, "New");
});

test("handleDeleteChat navigates only after the delete succeeded", () => {
  const src = read("../App.tsx");
  const body = src.slice(src.indexOf("const handleDeleteChat"));
  assert.ok(body.indexOf("await deleteChat(chatId)") < body.indexOf("go(next)"));
});
