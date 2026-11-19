import assert from "node:assert/strict";
import { test } from "node:test";
import { chatTitleLabel } from "./chatTitle.ts";

test("the daemon's default title is shown in the UI language, a real title is kept", () => {
  assert.equal(chatTitleLabel("New chat", "Yeni sohbet"), "Yeni sohbet");
  assert.equal(chatTitleLabel(undefined, "Yeni sohbet"), "Yeni sohbet");
  assert.equal(chatTitleLabel("  ", "Yeni sohbet"), "Yeni sohbet");
  assert.equal(chatTitleLabel("Trip plan", "Yeni sohbet"), "Trip plan");
});
