import assert from "node:assert/strict";
import { test } from "node:test";
import { markRemoteBaseUrl } from "../lib/liveViewState.ts";
import { ApiClient } from "./client.ts";

/** Records the headers of the one request a client makes. */
async function headersOf(client: ApiClient): Promise<Record<string, string>> {
  const real = globalThis.fetch;
  let seen: Record<string, string> = {};
  globalThis.fetch = (async (_url: unknown, init?: { headers?: Record<string, string> }) => {
    seen = init?.headers ?? {};
    return new Response("{}", { status: 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
  try {
    await client.health();
  } finally {
    globalThis.fetch = real;
  }
  return seen;
}

test("the owner key goes to this computer's daemon only, never through a host proxy", async () => {
  ApiClient.ownerKeyProvider = async () => "owner-key-for-test";
  try {
    const local = new ApiClient({ base_url: "http://127.0.0.1:47821", token: "t" });
    const proxy = new ApiClient({ base_url: "http://127.0.0.1:50999", token: "p" });
    const unmark = markRemoteBaseUrl("http://127.0.0.1:50999");
    try {
      // `health` is unauthenticated; use a call that sends the bearer path.
      const localHeaders = await headersOf(local);
      const proxyHeaders = await headersOf(proxy);
      assert.equal(localHeaders["x-praxi-owner-key"], "owner-key-for-test");
      assert.equal(proxyHeaders["x-praxi-owner-key"], undefined);
    } finally {
      unmark();
    }
  } finally {
    ApiClient.ownerKeyProvider = null;
  }
});
