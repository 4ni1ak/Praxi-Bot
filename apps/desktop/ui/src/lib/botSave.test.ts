import assert from "node:assert/strict";
import { test } from "node:test";
import type { Bot, BotInput } from "../api/types.ts";
import { draftFromBot, type BotDraft } from "./botProfile.ts";
import {
  changedProfileParts,
  mergeDraft,
  sameProfile,
  saveEditor,
  stableJson
} from "./botSave.ts";

const TARGETS: unknown[] = ["off", "browser", "host", "auto", "pc", "node:abc-123"];

function bot(config: Record<string, unknown>, name = "Atlas"): Bot {
  return { id: "b1", name, config, created_at_ms: 1 } as unknown as Bot;
}

const baseConfig = (runtime?: unknown): Record<string, unknown> => ({
  bot: { id: "atlas-1", name: "Atlas", persona: "Careful.", shape: "gem", color: "#4c8dff", unknown_bot_key: 7 },
  models: { executor: "gemini/gemini-3.5-flash-lite", fallback: ["custom/odd:model"], thinking_level: "high" },
  permissions: { preset: "approve-destructive" },
  tools: { exclude: ["shell"] },
  language: "tr",
  ...(runtime === undefined ? {} : { runtime: { computer: runtime, browser: runtime } })
});

/** A fake daemon: stores one bot, records every call. */
function fake(stored: { current: Bot }) {
  const calls: string[] = [];
  const puts: BotInput[] = [];
  return {
    calls,
    puts,
    async getBot() {
      calls.push("getBot");
      return structuredClone(stored.current);
    },
    async updateBot(_id: string, input: BotInput) {
      calls.push("updateBot");
      puts.push(structuredClone(input));
      stored.current = { ...stored.current, name: input.name ?? stored.current.name, config: input.config } as Bot;
      return structuredClone(stored.current);
    },
    async listMcpServers() {
      calls.push("listMcpServers");
      return [];
    },
    async updateMcpServer() {
      calls.push("updateMcpServer");
      throw new Error("not expected");
    }
  };
}

const args = (b: Bot, draft: BotDraft, over = {}) => ({
  baseline: b,
  draft,
  openedDraft: draftFromBot(b),
  profileDirty: true,
  initialApps: {},
  allowed: {},
  allBotIds: [b.id],
  ...over
});

test("a skills-only (or apps-only) Save never PUTs the bot, so it cannot touch the computer target", async () => {
  for (const target of TARGETS) {
    const b = bot(baseConfig(target));
    const api = fake({ current: b });
    let skillsSaved = 0;
    const result = await saveEditor(
      api,
      args(b, draftFromBot(b), {
        profileDirty: false,
        saveSkills: async () => {
          skillsSaved++;
          return { ok: true };
        }
      })
    );
    assert.equal(result.kind, "saved");
    assert.equal(skillsSaved, 1);
    assert.equal(api.puts.length, 0, `no profile write for runtime ${String(target)}`);
    assert.ok(!api.calls.includes("getBot"));
  }
});

test("a profile Save keeps the stored runtime of every target and every field it did not edit", async () => {
  for (const target of [...TARGETS, undefined]) {
    const b = bot(baseConfig(target));
    const stored = { current: b };
    const api = fake(stored);
    const draft = { ...draftFromBot(b), name: "Atlas 2" };
    const result = await saveEditor(api, args(b, draft));
    assert.equal(result.kind, "saved");
    const sent = api.puts[0]!.config as Record<string, unknown>;
    assert.equal(api.puts[0]!.name, "Atlas 2");
    assert.deepEqual(sent.runtime, (b.config as Record<string, unknown>).runtime);
    assert.equal("runtime" in sent, target !== undefined);
    // unknown fields round-trip untouched
    assert.deepEqual(sent.tools, { exclude: ["shell"] });
    assert.equal(sent.language, "tr");
    assert.deepEqual((sent.models as Record<string, unknown>).fallback, ["custom/odd:model"]);
    assert.equal((sent.models as Record<string, unknown>).thinking_level, "high");
    assert.equal(((sent.bot as Record<string, unknown>).unknown_bot_key), 7);
    assert.deepEqual(sent.permissions, { preset: "approve-destructive" });
  }
});

test("another client's change is a conflict: nothing is written, the form keeps its edit", async () => {
  const opened = bot(baseConfig("host"));
  const theirs = bot({ ...baseConfig("host"), permissions: { preset: "full" } });
  const api = fake({ current: theirs });
  const draft = { ...draftFromBot(opened), name: "Atlas 2" };
  const result = await saveEditor(api, args(opened, draft));
  assert.equal(result.kind, "conflict");
  if (result.kind !== "conflict") return;
  assert.deepEqual(result.changed, ["permissions"]);
  assert.equal(api.puts.length, 0);
});

test("overwrite lays only the user's edits over the stored profile (their permission change survives)", async () => {
  const opened = bot(baseConfig("host"));
  const theirs = bot({ ...baseConfig("host"), permissions: { preset: "full" } });
  const stored = { current: theirs };
  const api = fake(stored);
  const draft = { ...draftFromBot(opened), name: "Atlas 2" };
  const result = await saveEditor(api, args(opened, draft, { overwrite: true }));
  assert.equal(result.kind, "saved");
  const sent = api.puts[0]!.config as Record<string, unknown>;
  assert.equal(api.puts[0]!.name, "Atlas 2");
  assert.deepEqual(sent.permissions, { preset: "full" });
  assert.deepEqual((sent.runtime as Record<string, unknown>).computer, "host");
});

test("a skills conflict is reported, not hidden, and the profile write already happened once", async () => {
  const b = bot(baseConfig("off"));
  const api = fake({ current: b });
  const result = await saveEditor(
    api,
    args(b, { ...draftFromBot(b), persona: "New." }, {
      saveSkills: async () => ({ ok: false, failure: "conflict", message: "409" })
    })
  );
  assert.equal(result.kind, "saved");
  if (result.kind !== "saved") return;
  assert.equal(result.profileWritten, true);
  assert.deepEqual(result.skills, { ok: false, failure: "conflict", message: "409" });
  assert.equal(api.puts.length, 1);
});

test("profile comparison ignores key order and names the changed part", () => {
  const a = bot({ bot: { x: 1, y: 2 }, models: { executor: "a" } });
  const b = bot({ models: { executor: "a" }, bot: { y: 2, x: 1 } });
  assert.equal(sameProfile(a, b), true);
  assert.equal(stableJson({ b: 1, a: [2, { d: 1, c: 2 }] }), '{"a":[2,{"c":2,"d":1}],"b":1}');
  assert.deepEqual(
    changedProfileParts(a, bot({ ...(b.config as object), runtime: { computer: "off" }, extra: 1 }, "Other")),
    ["identity", "runtime", "other"]
  );
});

test("mergeDraft takes only what the user edited", () => {
  const opened = draftFromBot(bot(baseConfig()));
  const fresh = { ...opened, persona: "Theirs.", preset: "full" as const };
  const mine = { ...opened, name: "Mine" };
  const merged = mergeDraft(fresh, mine, opened);
  assert.equal(merged.name, "Mine");
  assert.equal(merged.persona, "Theirs.");
  assert.equal(merged.preset, "full");
});
