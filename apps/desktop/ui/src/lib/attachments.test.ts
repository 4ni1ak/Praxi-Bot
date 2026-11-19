import assert from "node:assert/strict";
import { test } from "node:test";
import { MAX_ATTACHMENTS, formatBytes, room, uploadErrorKey } from "./attachments.ts";

test("sizes are formatted", () => {
  assert.equal(formatBytes(300), "300 B");
  assert.equal(formatBytes(12 * 1024), "12 KB");
  assert.equal(formatBytes(1.5 * 1024 * 1024), "1.5 MB");
});

test("a message never exceeds the file limit", () => {
  assert.equal(room(MAX_ATTACHMENTS - 1, 3), 1);
  assert.equal(room(MAX_ATTACHMENTS, 2), 0);
});

test("the refusal reason follows the daemon code", () => {
  assert.equal(uploadErrorKey("unsupported_attachment", 415), "chat.attach.unsupported");
  assert.equal(uploadErrorKey(undefined, 413), "chat.attach.tooLarge");
  assert.equal(uploadErrorKey("network", 0), "chat.attach.failed");
});
