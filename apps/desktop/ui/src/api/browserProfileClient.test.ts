import assert from "node:assert/strict";
import { test } from "node:test";
import { ApiClient } from "./client.ts";
import { MockApiClient } from "./mock.ts";

test("browser profile actions stay bound to the selected bot and send no login credentials", async () => {
  const original = globalThis.fetch;
  const calls: { method: string; path: string; body: unknown }[] = [];
  globalThis.fetch = (async (url: URL | string, init?: RequestInit) => {
    calls.push({ method: init?.method ?? "GET", path: new URL(String(url)).pathname,
      body: init?.body ? JSON.parse(String(init.body)) : undefined });
    return new Response("{}", { status: 200 });
  }) as typeof fetch;
  try {
    const client = new ApiClient({ base_url: "http://127.0.0.1:9", token: "test" });
    await client.browserProfile("bot one");
    await client.startBrowserSignIn("bot one");
    await client.finishBrowserSignIn("bot one");
    await client.forgetBrowserSite("bot one", "example.org");
    await client.forgetBrowserProfile("bot one");
    const base = "/v1/bots/bot%20one/browser-profile";
    assert.deepEqual(calls, [
      { method: "GET", path: base, body: undefined },
      { method: "POST", path: `${base}/sign-in`, body: undefined },
      { method: "POST", path: `${base}/finish-sign-in`, body: undefined },
      { method: "POST", path: `${base}/forget-site`, body: { domain: "example.org" } },
      { method: "DELETE", path: base, body: undefined }
    ]);
  } finally {
    globalThis.fetch = original;
  }
});

test("MockApiClient isolates browser profiles per bot and tracks sign-in and forget actions", async () => {
  const mock = new MockApiClient();
  const bots = await mock.listBots();
  const [first, second] = bots;
  const initial = await mock.browserProfile(first.id);
  assert.deepEqual(initial.domains, ["github.com"]);
  assert.equal((await mock.browserProfile(second.id)).domains.length, 0);

  await mock.startBrowserSignIn(first.id);
  assert.equal((await mock.browserProfile(first.id)).signing_in, true);
  await assert.rejects(() => mock.forgetBrowserSite(first.id, "github.com"));
  await mock.finishBrowserSignIn(first.id);
  assert.equal((await mock.browserProfile(first.id)).signing_in, false);

  await mock.forgetBrowserSite(first.id, "github.com");
  assert.deepEqual((await mock.browserProfile(first.id)).domains, []);
  await mock.forgetBrowserProfile(first.id);
  assert.equal((await mock.browserProfile(first.id)).size_bytes, 0);
});
