import assert from "node:assert/strict";
import { test } from "node:test";
import { autoChatTitle, chatTitleLabel } from "./chatTitle.ts";

test("the daemon's default title is shown in the UI language, a real title is kept", () => {
  assert.equal(chatTitleLabel("New chat", "Yeni sohbet"), "Yeni sohbet");
  assert.equal(chatTitleLabel(undefined, "Yeni sohbet"), "Yeni sohbet");
  assert.equal(chatTitleLabel("  ", "Yeni sohbet"), "Yeni sohbet");
  assert.equal(chatTitleLabel("Trip plan", "Yeni sohbet"), "Trip plan");
});

test("an automatic title comes from the first line of the first message", () => {
  assert.equal(autoChatTitle("Create critic-notes.md in your workspace\nthen read it"),"Create critic-notes.md in your workspace");
  assert.equal(autoChatTitle("  **Plan** my trip!  "), "Plan my trip");
  const long = autoChatTitle("Write a long essay about the history of note taking applications and their features");
  assert.ok(long?.endsWith("…") && [...long].length <= 49);
  assert.equal(autoChatTitle("/new"), null);
  assert.equal(autoChatTitle("   "), null);
  assert.equal(autoChatTitle("***"), null);
});
