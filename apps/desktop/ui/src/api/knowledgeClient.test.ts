import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { ApiClient } from "./client.ts";

const realFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = realFetch; });

test("knowledge management binds every request to bot, source and reviewed revision", async () => {
  const calls: { method: string; path: string; body: unknown }[] = [];
  globalThis.fetch = (async (url: URL | string, input?: RequestInit) => {
    calls.push({ method: input?.method ?? "GET", path: new URL(String(url)).pathname, body: input?.body ? JSON.parse(String(input.body)) : undefined });
    return new Response("{}", { status: 200 });
  }) as typeof fetch;
  const client = new ApiClient({ base_url: "http://127.0.0.1:9", token: "test" });
  await client.listKnowledgeSources("bot one");
  await client.updateKnowledgeSource("bot one", "source/one", { action: "grant_remote", expected_revision: 7, label: null, profile_fingerprint: "a".repeat(64) });
  await client.reindexKnowledgeSource("bot one", "source/one", 8, "notes/b.md");
  await client.deleteKnowledgeSource("bot one", "source/one", 9);
  await client.embedKnowledgeSource("bot one", "source/one", 10);
  await client.embedKnowledgeSource("bot one", "source/one", 10, true);
  assert.equal(calls[0].path, "/v1/bots/bot%20one/knowledge/sources");
  assert.equal(calls[1].path, "/v1/bots/bot%20one/knowledge/sources/source%2Fone");
  assert.deepEqual(calls[1].body, { action: "grant_remote", expected_revision: 7, label: null, profile_fingerprint: "a".repeat(64) });
  assert.deepEqual(calls[2].body, { expected_revision: 8, cursor: "notes/b.md" });
  assert.deepEqual(calls[3].body, { expected_revision: 9 });
  assert.equal(calls[4].path, "/v1/bots/bot%20one/knowledge/sources/source%2Fone/embed");
  assert.deepEqual(calls[4].body, { expected_revision: 10, reset_retry: false });
  assert.deepEqual(calls[5].body, { expected_revision: 10, reset_retry: true });
});
