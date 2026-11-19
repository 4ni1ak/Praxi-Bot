import assert from "node:assert/strict";
import { test } from "node:test";
import { copyText, visibleMessageText } from "./copyText.ts";

test("copyText copies visible plain text and markdown and refuses empty or denied clipboard", async () => {
  let written = "";
  assert.equal(await copyText("  **Hello** world  ", { writeText: async (s) => void (written = s) }), true);
  assert.equal(written, "**Hello** world");
  assert.equal(await copyText("  ", { writeText: async () => undefined }), false);
  assert.equal(
    await copyText("x", {
      writeText: async () => {
        throw new Error("denied");
      }
    }),
    false
  );
});

test("visibleMessageText strips internal markers and suppresses raw internal tool JSON while keeping normal JSON", async () => {
  assert.equal(
    visibleMessageText("Please check this\n\n[[praxi-attachments:11111111-1111-4111-8111-111111111111]]"),
    "Please check this"
  );
  assert.equal(
    visibleMessageText('{"tool_calls":[{"id":"c1","name":"shell_run","arguments":"{}"}]}'),
    ""
  );
  assert.equal(
    visibleMessageText('[{"type":"tool_use","id":"t1","name":"fs_read","input":{"path":"a.txt"}}]'),
    ""
  );
  assert.equal(
    visibleMessageText('{"type":"tool_result","tool_call_id":"c1","content":"secret"}'),
    ""
  );
  assert.equal(
    visibleMessageText('Done.<tool_call>{"name":"web_search"}</tool_call>'),
    "Done."
  );
  // Legitimate JSON answer requested by the user stays copyable.
  assert.equal(visibleMessageText('{"port": 8080, "host": "localhost"}'), '{"port": 8080, "host": "localhost"}');
  let called = false;
  assert.equal(
    await copyText('{"tool_call_id":"c1","content":"raw"}', {
      writeText: async () => {
        called = true;
      }
    }),
    false
  );
  assert.equal(called, false);
});
