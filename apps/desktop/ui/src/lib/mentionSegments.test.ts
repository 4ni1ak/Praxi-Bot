import assert from "node:assert/strict";
import { test } from "node:test";
import { mentionSegments } from "./slash.ts";

test("only whole @mentions of known bots are coloured", () => {
  const bots = [{ name: "Nova", color: "#2fc4b2" }, { name: "Atlas Pro", color: "#8b9cff" }];
  assert.deepEqual(mentionSegments("hi @nova and @AtlasPro, mail a@nova.com @Ghost", bots), [
    { text: "hi " },
    { text: "@nova", color: "#2fc4b2" },
    { text: " and " },
    { text: "@AtlasPro", color: "#8b9cff" },
    { text: ", mail a@nova.com @Ghost" }
  ]);
  assert.deepEqual(mentionSegments("", bots), []);
});
