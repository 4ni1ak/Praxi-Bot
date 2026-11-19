import assert from "node:assert/strict";
import { test } from "node:test";
import { chatActivityMs, emptyReadState, firstUnseenMessageId, isAttended, unreadBotIds, withRead } from "./unread.ts";

const chats = [{ id: "c1", bot_id: "b1" }, { id: "c2", bot_id: "b2" }];
const run = (chat: string, state: string, at: number) => ({ chat_id: chat, state, updated_at_ms: at });

test("a bot is unread only for finished runs or waiting decisions after the last read", () => {
  const s = withRead(emptyReadState(100), "c2", 500);
  const runs = [run("c1", "succeeded", 200), run("c2", "succeeded", 400), run("c2", "running", 900)];
  assert.deepEqual([...unreadBotIds(chats, runs, [], s)], ["b1"], "c2 was read past its run; a live run is not a message yet");
  assert.deepEqual([...unreadBotIds(chats, [], [{ chatId: "c2", sinceMs: 700 }], s)], ["b2"], "an approval waiting counts");
  assert.equal(unreadBotIds(chats, [run("c1", "succeeded", 50)], [], s).size, 0, "older than the first-sight baseline: read");
});

test("the read position only moves forward and ignores sub-agent runs", () => {
  const s = withRead(emptyReadState(0), "c1", 300);
  assert.equal(withRead(s, "c1", 100), s);
  assert.equal(chatActivityMs([{ ...run("c1", "succeeded", 900), parent_run_id: "p" }], [], "c1"), 0);
});

test("the divider sits above the first unseen bot message, not the user's own", () => {
  const m = (id: string, role: string, at: number) => ({ id, role, created_at_ms: at });
  const list = [m("1", "assistant", 10), m("2", "user", 30), m("3", "assistant", 40), m("4", "assistant", 50)];
  assert.equal(firstUnseenMessageId(list, 20), "3");
  assert.equal(firstUnseenMessageId(list, 50), null);
});

test("nothing counts as read while the window is hidden or unfocused", () => {
  assert.equal(isAttended({ visibilityState: "hidden", hasFocus: () => true }), false);
  assert.equal(isAttended({ visibilityState: "visible", hasFocus: () => false }), false);
  assert.equal(isAttended({ visibilityState: "visible", hasFocus: () => true }), true);
});
