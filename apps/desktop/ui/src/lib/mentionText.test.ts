import assert from "node:assert/strict";
import { test } from "node:test";
import { splitMentions } from "./mentionText.ts";

test("@Atlas is a coloured part; emails, unknown names and longer words are not", () => {
  const bots = [{ name: "Atlas", color: "#3b82f6" }, { name: "Atlas Pro", color: "#ef4444" }];
  assert.deepEqual(splitMentions("@Atlas sen nasılsın", bots), [
    { text: "@Atlas", color: "#3b82f6" },
    { text: " sen nasılsın" }
  ]);
  assert.deepEqual(splitMentions("hi @atlaspro, and a@Atlas", bots), [
    { text: "hi " },
    { text: "@atlaspro", color: "#ef4444" },
    { text: ", and a@Atlas" }
  ]);
  assert.deepEqual(splitMentions("@Atlasx @Nobody", bots), [{ text: "@Atlasx @Nobody" }]);
});
