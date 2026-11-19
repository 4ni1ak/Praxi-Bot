import assert from "node:assert/strict";
import { test } from "node:test";
import { markdownToSpeech } from "./speechText.ts";

const labels = { code: "(code omitted)", table: "(table omitted)" };

test("code and tables are summarised, formatting marks removed", () => {
  const md = [
    "# Result",
    "",
    "**Done**: see [the docs](https://example.com/x) and `run`.",
    "- first",
    "- second",
    "```ts",
    "const secret = 1;",
    "```",
    "| a | b |",
    "|---|---|",
    "| 1 | 2 |",
    "Bye"
  ].join("\n");
  assert.equal(
    markdownToSpeech(md, labels),
    "Result\nDone: see the docs and run.\nfirst\nsecond\n(code omitted)\n(table omitted)\nBye"
  );
});
