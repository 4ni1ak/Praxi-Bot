import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { ApiClient } from "./client.ts";
import { MockApiClient } from "./mock.ts";

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

test("memory client encodes filters, mutations, undo and recall preview routes", async () => {
  const calls: { method: string; path: string; search: string; body: unknown }[] = [];
  globalThis.fetch = (async (url: URL | string, input?: RequestInit) => {
    const parsed = new URL(String(url));
    calls.push({
      method: input?.method ?? "GET",
      path: parsed.pathname,
      search: parsed.search,
      body: input?.body ? JSON.parse(String(input.body)) : undefined
    });
    return new Response("{}", { status: 200 });
  }) as typeof fetch;
  const client = new ApiClient({ base_url: "http://127.0.0.1:9", token: "test" });
  await client.listMemories({
    bot: "bot-1",
    botOnly: true,
    kind: "lesson",
    q: "release",
    includeDeleted: true
  });
  await client.createMemory({ bot_id: "bot-1", kind: "bot", content: "Deploys on Tuesdays." });
  await client.updateMemory("mem/1", { content: "Deploys on Wednesdays." });
  await client.deleteMemory("mem/1");
  await client.undoMemory("mem/1");
  await client.previewBotMemory("bot/1", "Tuesday deploy");

  assert.equal(calls[0].method, "GET");
  assert.equal(calls[0].path, "/v1/memory");
  assert.equal(
    calls[0].search,
    "?bot=bot-1&bot_only=true&kind=lesson&q=release&include_deleted=true"
  );
  assert.deepEqual(calls.slice(1), [
    {
      method: "POST",
      path: "/v1/memory",
      search: "",
      body: { bot_id: "bot-1", kind: "bot", content: "Deploys on Tuesdays." }
    },
    {
      method: "PATCH",
      path: "/v1/memory/mem%2F1",
      search: "",
      body: { content: "Deploys on Wednesdays." }
    },
    { method: "DELETE", path: "/v1/memory/mem%2F1", search: "", body: undefined },
    { method: "POST", path: "/v1/memory/mem%2F1/undo", search: "", body: undefined },
    {
      method: "GET",
      path: "/v1/bots/bot%2F1/memory/preview",
      search: "?query=Tuesday+deploy",
      body: undefined
    }
  ]);
});

test("mock memory client isolates bots and supports forget and undo", async () => {
  const mock = new MockApiClient();
  const bots = await mock.listBots();
  const atlas = bots[0].id;
  const nova = bots[1].id;

  const created = await mock.createMemory({
    bot_id: atlas,
    kind: "lesson",
    content: "Always check staging health before promotion."
  });
  assert.equal(created.bot_id, atlas);

  const atlasOnly = await mock.listMemories({ bot: atlas, botOnly: true });
  assert.ok(atlasOnly.some((m) => m.id === created.id));

  const novaList = await mock.listMemories({ bot: nova });
  assert.ok(!novaList.some((m) => m.id === created.id));

  const forgotten = await mock.deleteMemory(created.id);
  assert.equal(forgotten.deleted, true);

  const restored = await mock.undoMemory(created.id);
  assert.equal(restored.deleted, false);

  const preview = await mock.previewBotMemory(atlas, "staging health");
  assert.ok(preview.rendered_block.includes("<recalled_memory>"));
  assert.ok(preview.rendered_block.includes("Always check staging health before promotion."));
});
