import assert from "node:assert/strict";
import { test } from "node:test";
import { botsHome } from "./botsHome.ts";

function fakes() {
  const calls: string[] = [];
  const rec = (n: string) => async () => {
    calls.push(n);
    return {} as never;
  };
  return {
    calls,
    local: { createBot: rec("local.createBot"), updateBot: rec("local.updateBot") },
    localProviders: {
      saveProvider: async () => {
        calls.push("local.saveProvider");
      },
      setDefault: async () => {
        calls.push("local.setDefault");
      }
    },
    host: {
      adminPutProvider: rec("host.putProvider"),
      adminSetDefault: rec("host.setDefault"),
      createBot: rec("host.createBot"),
      updateBot: rec("host.updateBot")
    }
  };
}

test("writes go to the adopted host, else to this computer", async () => {
  const f = fakes();
  const withHost = botsHome({ local: f.local, localProviders: f.localProviders, host: f.host });
  await withHost.saveProvider("gemini", { apiKey: "k", makeDefault: true });
  await withHost.createBot({});
  assert.equal(withHost.target, "host");
  assert.deepEqual(f.calls, ["host.putProvider", "host.createBot"]);
  f.calls.length = 0;
  const alone = botsHome({ local: f.local, localProviders: f.localProviders, host: null });
  await alone.saveProvider("gemini", { apiKey: "k", makeDefault: true });
  await alone.updateBot("b", {});
  assert.deepEqual(f.calls, ["local.saveProvider", "local.setDefault", "local.updateBot"]);
});

test("a provider add goes to the home as a write-only key; nothing is written locally and no owner key is sent", async () => {
  const f = fakes();
  const seen: unknown[][] = [];
  const host = { ...f.host, adminPutProvider: async (...args: unknown[]) => { seen.push(args); return {} as never; } };
  await botsHome({ local: f.local, localProviders: f.localProviders, host }).saveProvider("gemini", {
    apiKey: "AIza-secret",
    model: "gemini-3.5-flash-lite",
    makeDefault: true
  });
  assert.deepEqual(seen, [["gemini", { api_key: "AIza-secret", base_url: undefined, model: "gemini-3.5-flash-lite", make_default: true }]]);
  assert.deepEqual(f.calls, []);
  // The host call takes (id, input) only: this computer's owner key never travels to the host.
  assert.equal(seen[0]?.length, 2);
});
