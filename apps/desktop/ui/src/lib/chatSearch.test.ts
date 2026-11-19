import assert from "node:assert/strict";
import { test } from "node:test";
import { filterChats } from "./chatSearch.ts";

const chats = [{ title: "Trip to Rome" }, { title: "Tax return" }, { title: "" }];

test("filters by displayed title, ignoring case, and keeps all for a blank query", () => {
  assert.deepEqual(filterChats(chats, "ROME", "New chat"), [chats[0]]);
  assert.equal(filterChats(chats, "  ", "New chat").length, 3);
  assert.deepEqual(filterChats(chats, "zzz", "New chat"), []);
});

test("an untitled chat is found by its placeholder label", () => {
  assert.deepEqual(filterChats(chats, "new", "New chat"), [chats[2]]);
});
