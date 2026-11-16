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
