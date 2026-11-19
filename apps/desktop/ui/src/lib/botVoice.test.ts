import assert from "node:assert/strict";
import { test } from "node:test";
import { effectiveVoice, getBotVoice, setBotVoice } from "./botVoice.ts";

test("a bot override wins over the global voice and can be removed", () => {
  const store = new Map<string, string>();
  (globalThis as { window?: unknown }).window = {
    localStorage: {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v)
    }
  };
  const global = { voiceName: "G", rate: 1 };
  assert.deepEqual(effectiveVoice("b1", global), global);
  setBotVoice("b1", { voiceName: "Bot", rate: 9 });
  assert.deepEqual(getBotVoice("b1"), { voiceName: "Bot", rate: 2 });
  assert.equal(effectiveVoice("b1", global).voiceName, "Bot");
  assert.equal(effectiveVoice("b2", global).voiceName, "G");
  setBotVoice("b1", null);
  assert.equal(effectiveVoice("b1", global).voiceName, "G");
});
