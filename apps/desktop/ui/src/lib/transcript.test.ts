import assert from "node:assert/strict";
import { test } from "node:test";
import { transcriptFileName, transcriptMarkdown } from "./transcript.ts";

test("the transcript lists user and bot messages with times and skips other roles", () => {
  const md = transcriptMarkdown(
    [
      { role: "user", content: "Hi", created_at_ms: Date.UTC(2026, 0, 2, 3, 4, 5) },
      { role: "tool", content: "secret tool output", created_at_ms: 1 },
      { role: "assistant", content: "Hello!", created_at_ms: Date.UTC(2026, 0, 2, 3, 4, 9) }
    ],
    { title: "Plan", user: "You", bot: "Atlas" }
  );
  assert.equal(md, "# Plan\n\n## You · 2026-01-02 03:04:05 UTC\n\nHi\n\n## Atlas · 2026-01-02 03:04:09 UTC\n\nHello!\n");
  assert.equal(transcriptFileName("Plan: a/b?"), "Plan-a-b.md");
  assert.equal(transcriptFileName("???"), "chat.md");
});
