import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { ApiError } from "../api/client.ts";
import type { Bot, BotPatchInput, McpServer } from "../api/types.ts";
import { applyPatchOps, conflictBot, profilePatch } from "./botPatch.ts";
import { type BotDraft, draftFromBot } from "./botProfile.ts";
import {
  changedProfileParts,
  type EditorSaveInput,
  performEditorSave,
  stableJson
} from "./botSave.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");

const TARGETS = ["off", "browser", "host", "auto", "pc", "node:abc-123"];

function mkBot(config: Record<string, unknown>, name = "Atlas", version = 1): Bot {
  return { id: "b1", name, config, created_at_ms: 1, version } as unknown as Bot;
}

const baseConfig = (runtime?: unknown): Record<string, unknown> => ({
  bot: { id: "atlas-1", name: "Atlas", persona: "Careful.", shape: "gem", color: "#4c8dff", unknown_bot_key: 7 },
  models: { executor: "gemini/gemini-3.5-flash-lite", fallback: ["custom/odd:model"], thinking_level: "high" },
  permissions: { preset: "approve-destructive" },
  tools: { exclude: ["shell"] },
  language: "tr",
  ...(runtime === undefined ? {} : { runtime: { computer: runtime, browser: runtime } })
});

/**
 * A fake daemon with the real route's semantics: one stored bot with a version, `patchBot`
 * applies the ops atomically or answers 409 with the current bot, and every call is recorded.
 * `hold` makes the next patch wait (a slow request).
 */
function fakeDaemon(initial: Bot) {
  const stored = { current: structuredClone(initial) };
  const patches: BotPatchInput[] = [];
  const calls: string[] = [];
  const servers: McpServer[] = [];
  const gate: { wait: Promise<void> | null; fail: Error | null } = { wait: null, fail: null };
  const client = {
    async patchBot(_id: string, input: BotPatchInput) {
      calls.push("patchBot");
      patches.push(structuredClone(input));
      if (gate.wait) await gate.wait;
      if (gate.fail) throw gate.fail;
      if (input.expected_version !== stored.current.version) {
        throw new ApiError(409, "version_conflict", "stale", {
          error: { code: "version_conflict", message: "stale" },
          current: structuredClone(stored.current)
        });
      }
      stored.current = {
        ...stored.current,
        name: input.name ?? stored.current.name,
        config: applyPatchOps(stored.current.config, input.ops as never),
        version: stored.current.version + 1
      } as Bot;
      return structuredClone(stored.current);
    },
    async listMcpServers() {
      calls.push("listMcpServers");
      return structuredClone(servers);
    },
    async updateMcpServer(name: string, patch: { bots_add?: string[]; bots_remove?: string[] }) {
      calls.push(`updateMcpServer:${name}`);
      const srv = servers.find((s) => s.name === name);
      if (!srv) throw new Error("no such server");
      const bots = new Set(srv.bots ?? []);
      for (const b of patch.bots_add ?? []) bots.add(b);
      for (const b of patch.bots_remove ?? []) bots.delete(b);
      srv.bots = [...bots];
      return structuredClone(srv);
    }
  };
  return { stored, patches, calls, servers, gate, client };
}

function input(
  daemon: ReturnType<typeof fakeDaemon>,
  baseline: Bot,
  edit: (d: BotDraft) => BotDraft,
  over: Partial<EditorSaveInput> = {}
): EditorSaveInput {
  const opened = draftFromBot(baseline);
  return {
    client: daemon.client as never,
    baseline,
    draft: edit(opened),
    openedDraft: opened,
    problems: [],
    initialApps: {},
    allowed: {},
    allBotIds: [baseline.id],
    skills: { payload: null, save: async () => ({ ok: true }) },
    ...over
  };
}

// ---- finding 1 -------------------------------------------------------------------------------

test("a skills-only Save sends no profile request, whatever the computer target", async () => {
  for (const target of TARGETS) {
    const b = mkBot(baseConfig(target));
    const d = fakeDaemon(b);
    let saved = 0;
    const out = await performEditorSave(
      input(d, b, (x) => x, {
        skills: {
          payload: { mode: "all", expected_version: 1 },
          save: async () => {
            saved++;
            return { ok: true };
          }
        }
      })
    );
    assert.equal(out.kind, "saved");
    assert.equal(saved, 1);
    assert.deepEqual(d.calls, [], `no request but the skills one for ${target}`);
  }
});

test("an apps-only Save executes the app change and sends no profile request", async () => {
  const b = mkBot(baseConfig("host"));
  const d = fakeDaemon(b);
  d.servers.push({ name: "github", status: "connected", bots: ["other"] } as unknown as McpServer);
  const out = await performEditorSave(
    input(d, b, (x) => x, { initialApps: { github: false }, allowed: { github: true } })
  );
  assert.equal(out.kind, "saved");
  assert.ok(d.calls.some((c) => c.startsWith("updateMcpServer:github")), d.calls.join());
  assert.ok(!d.calls.includes("patchBot"));
  assert.deepEqual(d.servers[0]?.bots?.sort(), ["b1", "other"]);
});

test("a model-only change on a sparse profile writes exactly that one field", async () => {
  // bot = {id,name}, no planner, no fallback: nothing may be added or removed besides the executor.
  const b = mkBot({ bot: { id: "a", name: "A" }, models: { executor: "x/y" } });
  const d = fakeDaemon(b);
  const out = await performEditorSave(
    input(d, b, (x) => ({ ...x, executor: { provider: "x", model: "z" } }))
  );
  assert.equal(out.kind, "saved");
  assert.deepEqual(d.patches[0]?.ops, [{ op: "set", path: "/models/executor", value: "x/z" }]);
  assert.equal(d.patches[0]?.name, undefined);
  assert.deepEqual(d.stored.current.config, { bot: { id: "a", name: "A" }, models: { executor: "x/z" } });
});

test("every kind of profile edit becomes only its own op; unknown keys and runtime are untouched", async () => {
  for (const target of [...TARGETS, undefined]) {
    const b = mkBot(baseConfig(target));
    const d = fakeDaemon(b);
    const out = await performEditorSave(input(d, b, (x) => ({ ...x, name: "Atlas 2", persona: "Bold." })));
    assert.equal(out.kind, "saved");
    assert.deepEqual(
      d.patches[0]?.ops?.map((o) => o.path).sort(),
      ["/bot/name", "/bot/persona"]
    );
    const after = d.stored.current.config as Record<string, unknown>;
    assert.deepEqual(after.runtime, (b.config as Record<string, unknown>).runtime);
    assert.deepEqual(after.tools, { exclude: ["shell"] });
    assert.equal(after.language, "tr");
    assert.deepEqual((after.models as Record<string, unknown>).fallback, ["custom/odd:model"]);
    assert.deepEqual(after.permissions, { preset: "approve-destructive" });
  }
});

test("permission ops are one rule per edited category; a preset choice replaces the policy", () => {
  const b = mkBot({ ...baseConfig(), permissions: { categories: { fs: { level: { level: "ask" } } } } });
  const opened = draftFromBot(b);
  const edited = { ...opened, categories: { ...opened.categories, fs: "deny" as const } };
  assert.deepEqual(profilePatch(opened, edited).ops, [
    { op: "set", path: "/permissions/categories/fs/level", value: { level: "deny" } }
  ]);
  assert.deepEqual(profilePatch(opened, { ...opened, preset: "full" }).ops, [
    { op: "set", path: "/permissions", value: { preset: "full" } }
  ]);
  assert.deepEqual(profilePatch(opened, opened).ops, []);
});

// ---- finding 2 / 16: concurrency ---------------------------------------------------------------

test("a change that lands while the Save request is in flight is a conflict, not a lost update", async () => {
  const b = mkBot(baseConfig("off"));
  const d = fakeDaemon(b);
  let release: () => void = () => {};
  d.gate.wait = new Promise((r) => {
    release = r;
  });
  const pending = performEditorSave(input(d, b, (x) => ({ ...x, name: "Mine" })));
  // Client B writes (computer target and a permission) before A's request is applied.
  d.stored.current = mkBot(
    { ...baseConfig("host"), permissions: { preset: "read-only" } },
    "Atlas",
    2
  );
  release();
  const out = await pending;
  assert.equal(out.kind, "conflict");
  if (out.kind !== "conflict") return;
  assert.equal(out.fresh.version, 2);
  assert.deepEqual(out.parts, ["permissions", "runtime"]);
  // Nothing of A's reached the stored bot: B's runtime and permission are intact.
  assert.equal(d.stored.current.name, "Atlas");
  assert.deepEqual((d.stored.current.config as Record<string, unknown>).runtime, {
    computer: "host",
    browser: "host"
  });
});

test("Overwrite re-sends only the user's edited fields: the unedited shell restriction survives", async () => {
  const cfg = {
    ...baseConfig(),
    permissions: { categories: { fs: { level: { level: "ask" } }, shell: { level: { level: "ask" } } } }
  };
  const b = mkBot(cfg);
  const d = fakeDaemon(b);
  // A edits only fs -> deny; B has already set shell -> deny (version 2).
  d.stored.current = mkBot(
    { ...cfg, permissions: { categories: { fs: { level: { level: "ask" } }, shell: { level: { level: "deny" } } } } },
    "Atlas",
    2
  );
  const edit = (x: BotDraft): BotDraft => ({ ...x, categories: { ...x.categories, fs: "deny" } });
  const first = await performEditorSave(input(d, b, edit));
  assert.equal(first.kind, "conflict");
  if (first.kind !== "conflict") return;
  assert.deepEqual(changedProfileParts(b, first.fresh), ["permissions"]);

  const second = await performEditorSave(input(d, b, edit, { expectedVersion: first.fresh.version }));
  assert.equal(second.kind, "saved");
  const sent = d.patches.at(-1);
  assert.equal(sent?.expected_version, 2);
  assert.deepEqual(sent?.ops?.map((o) => o.path), ["/permissions/categories/fs/level"]);
  const cats = (d.stored.current.config as { permissions: { categories: Record<string, { level: { level: string } }> } })
    .permissions.categories;
  assert.equal(cats.fs?.level.level, "deny");
  assert.equal(cats.shell?.level.level, "deny");
});

test("Overwrite hits a second conflict if the bot changed again meanwhile", async () => {
  const b = mkBot(baseConfig());
  const d = fakeDaemon(b);
  d.stored.current = mkBot(baseConfig(), "Atlas", 3);
  const out = await performEditorSave(input(d, b, (x) => ({ ...x, name: "M" }), { expectedVersion: 2 }));
  assert.equal(out.kind, "conflict");
});

// ---- the rest of the Save path (finding 15) -------------------------------------------------------

test("problems and the skills limit block Save before any request", async () => {
  const b = mkBot(baseConfig());
  const d = fakeDaemon(b);
  const blocked = await performEditorSave(
    input(d, b, (x) => x, { problems: [{ problem: "nameMissing", section: "identity" }] })
  );
  assert.deepEqual(blocked, { kind: "blocked", section: "identity" });
  const names = Array.from({ length: 201 }, (_, i) => `s${i}`);
  const tooMany = await performEditorSave(
    input(d, b, (x) => x, { skills: { payload: { mode: "selected", names, expected_version: 1 }, save: async () => ({ ok: true }) } })
  );
  assert.equal(tooMany.kind, "blocked");
  assert.deepEqual(d.calls, []);
});

test("a rejected profile request is a failure with the message, nothing else runs, and a retry works", async () => {
  const b = mkBot(baseConfig());
  const d = fakeDaemon(b);
  d.gate.fail = new Error("network down");
  let skillsRan = 0;
  const run = () =>
    performEditorSave(
      input(d, b, (x) => ({ ...x, name: "Mine" }), {
        skills: { payload: { mode: "all", expected_version: 1 }, save: async () => { skillsRan++; return { ok: true }; } }
      })
    );
  const failed = await run();
  assert.deepEqual(failed, { kind: "failed", message: "network down" });
  assert.equal(skillsRan, 0);
  d.gate.fail = null;
  const retried = await run();
  assert.equal(retried.kind, "saved");
  assert.equal(skillsRan, 1);
  assert.equal(d.stored.current.name, "Mine");
});

test("partial success: the profile is written, a failing app step and a skills conflict are reported, the dialog stays", async () => {
  const b = mkBot(baseConfig());
  const d = fakeDaemon(b);
  d.servers.push({ name: "github", status: "connected", bots: [] } as unknown as McpServer);
  d.client.updateMcpServer = async () => {
    throw new Error("server error");
  };
  const out = await performEditorSave(
    input(d, b, (x) => ({ ...x, persona: "P" }), {
      initialApps: { github: false },
      allowed: { github: true },
      skills: { payload: { mode: "all", expected_version: 1 }, save: async () => ({ ok: false, failure: "conflict", message: "409" }) }
    })
  );
  assert.equal(out.kind, "saved");
  if (out.kind !== "saved") return;
  assert.equal(out.profileWritten, true);
  assert.equal(out.close, false);
  assert.deepEqual(out.messages.map((m) => m.key), ["botForm.apps.partialFail", "bots.skills.conflict"]);
  assert.equal(out.bot.version, 2);
  // The failed tick stays unsaved so a retry resends it.
  assert.equal(out.initialApps.github, false);
});

test("the dialog hands everything to performEditorSave and keeps no save logic of its own", () => {
  const dialog = read("../views/BotEditModal.tsx");
  assert.match(dialog, /performEditorSave\(\{/);
  assert.doesNotMatch(dialog, /applyAppsSelection|patchBot\(|updateBot\(|saveBotAndApps/);
  assert.match(dialog, /save\(conflict\.fresh\.version\)/);
});

test("profile comparison ignores key order and names the changed part", () => {
  const a = mkBot({ bot: { x: 1, y: 2 }, models: { executor: "a" } });
  const b = mkBot({ models: { executor: "a" }, bot: { y: 2, x: 1 } });
  assert.deepEqual(changedProfileParts(a, b), []);
  assert.equal(stableJson({ b: 1, a: [2, { d: 1, c: 2 }] }), '{"a":[2,{"c":2,"d":1}],"b":1}');
  assert.deepEqual(
    changedProfileParts(a, mkBot({ ...(b.config as object), runtime: { computer: "off" }, extra: 1 }, "Other")),
    ["identity", "runtime", "other"]
  );
});

test("only a 409 with a current bot is a conflict", () => {
  const cur = mkBot({});
  assert.equal(conflictBot(new ApiError(409, "version_conflict", "x", { current: cur })), cur);
  assert.equal(conflictBot(new ApiError(409, "conflict", "x", { current: cur })), null);
  assert.equal(conflictBot(new ApiError(500, "x", "x")), null);
  assert.equal(conflictBot(new Error("x")), null);
});
