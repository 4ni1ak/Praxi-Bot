import assert from "node:assert/strict";
import { test } from "node:test";
import { ApiError } from "../api/client.ts";
import type { Bot } from "../api/types.ts";
import { botWriteMessage, writeBotProfile } from "./botWrite.ts";

const bot = (version: number) => ({ id: "b1", name: "Atlas", version, config: {} }) as unknown as Bot;

test("a stale bot version reloads once and retries with the fresh version; a second conflict is a human sentence", async () => {
  const sent: Array<number | null | undefined> = [];
  let failures = 1;
  const client = {
    getBot: async () => bot(7),
    updateBot: async (_id: string, input: { expected_version?: number | null }) => {
      sent.push(input.expected_version);
      if (failures-- > 0) throw new ApiError(409, "version_conflict", "the bot changed since `expected_version`");
      return bot(8);
    }
  };
  const saved = await writeBotProfile(client, bot(3), (b) => ({ name: b.name, config: {} }) as never);
  assert.equal(saved.version, 8);
  assert.deepEqual(sent, [3, 7]);

  failures = 5;
  sent.length = 0;
  await assert.rejects(writeBotProfile(client, bot(3), () => ({ name: "x", config: {} }) as never), (e) => {
    const text = botWriteMessage(e, (k) => `tr:${k}`);
    assert.equal(text, "tr:bots.writeConflict");
    assert.doesNotMatch(text, /expected_version/);
    return true;
  });
  assert.equal(sent.length, 2, "exactly one retry");
});
