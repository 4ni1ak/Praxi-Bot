import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { ApiClient } from "./client.ts";

const originalFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = originalFetch; });
const client = () => new ApiClient({ base_url: "http://127.0.0.1:9", token: "owner" });

test("CLI preview sends only engine id; never command or argv", async () => {
  const calls: { url: string; body: unknown }[] = [];
  globalThis.fetch = (async (url: URL | string, init?: RequestInit) => {
    assert.equal(new Headers(init?.headers).get("authorization"), "Bearer owner");
    calls.push({ url: String(url), body: init?.body ? JSON.parse(String(init.body)) : null });
    return Response.json({ approval_id: "approval", command: "fixed" });
  }) as typeof fetch;
  await client().cliEngines(true);
  await client().previewCliInstall("opencode");
  assert.equal(new URL(calls[0].url).searchParams.get("refresh"), "1");
  assert.equal(new URL(calls[1].url).pathname, "/v1/providers/cli/install");
  assert.deepEqual(calls[1].body, { id: "opencode" });
});

test("install streams split output as text and requires a final result", async () => {
  const chunks = ["event: out", "put\ndata: {\"text\":\"<script>untrusted</script>\"}\n\n", "event: done\ndata: {\"ok\":true}\n\n"];
  globalThis.fetch = (async (_url: unknown, init?: RequestInit) => {
    assert.deepEqual(JSON.parse(String(init?.body)), { decision: "approve" });
    return new Response(new ReadableStream({ start(controller) {
      for (const text of chunks) { controller.enqueue(new TextEncoder().encode(text)); }
      controller.close();
    } }));
  }) as typeof fetch;
  const output: string[] = [];
  await client().decideCliInstall("approval", "approve", (text) => output.push(text));
  assert.deepEqual(output, ["<script>untrusted</script>"]);
  chunks.pop();
  await assert.rejects(client().decideCliInstall("approval", "approve", () => {}), /disconnected/);
});

test("installer timeout and denial are distinguished", async () => {
  let error = "timeout";
  globalThis.fetch = (async () => new Response(`event: done\ndata: ${JSON.stringify({ ok: false, error })}\n\n`)) as typeof fetch;
  await assert.rejects(client().decideCliInstall("approval", "approve", () => {}), /timeout/);
  error = "denied";
  await client().decideCliInstall("approval", "deny", () => {});
});
