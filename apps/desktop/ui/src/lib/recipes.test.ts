import assert from "node:assert/strict";
import { test } from "node:test";
import type { McpCatalogEntry } from "../api/types.ts";
import {
  applyRecipeExtras,
  type Recipe,
  recipeLines,
  recipeScheduleInput,
  recipeSpec,
  recipeText
} from "./recipes.ts";

const three = (en: string) => ({ en, tr: `${en} tr`, de: `${en} de` });

const recipe = {
  format: "praxi-recipe",
  format_version: 1,
  id: "daily-briefing",
  icon: "sun",
  names: three("Briefing"),
  descriptions: three("Short"),
  level: "approve-destructive",
  can: { en: ["a"], tr: ["b"], de: ["c"] },
  cannot: { en: ["x"], tr: ["y"], de: ["z"] },
  apps: [
    { catalog_id: "fetch", why: three("read pages") },
    { catalog_id: "brave-search", why: three("search") }
  ],
  schedule: {
    title: three("Morning"),
    prompt: three("Do it"),
    freq: "weekdays",
    time: "08:30"
  },
  profile: { format: "praxi-bot-profile", format_version: 1, bot: { name: "B", persona: "" } }
} as unknown as Recipe;

const entry = (id: string): McpCatalogEntry =>
  ({
    id,
    name: id,
    description: "",
    category: "remote",
    homepage: "",
    install: { transport: "stdio", command: "npx", args: ["x"], required_secrets: [] }
  }) as unknown as McpCatalogEntry;

function fakeApi(existing: string[], failApp?: string) {
  const calls = { servers: [] as unknown[], schedules: [] as unknown[] };
  return {
    calls,
    api: {
      listMcpServers: async () => existing.map((name) => ({ name })) as never,
      mcpCatalog: async () => [entry("fetch"), entry("brave-search")],
      createMcpServer: async (c: { name: string }) => {
        if (c.name === failApp) throw new Error("boom");
        calls.servers.push(c);
        return {};
      },
      createSchedule: async (s: unknown) => {
        calls.schedules.push(s);
        return {};
      }
    }
  };
}

test("text falls back to English and lines follow the language", () => {
  assert.equal(recipeText(recipe.names, "tr-TR"), "Briefing tr");
  assert.equal(recipeText({ en: "only" }, "de"), "only");
  assert.deepEqual(recipeLines(recipe.can, "de"), ["c"]);
});

test("the schedule becomes a cron line in the person's zone", () => {
  assert.deepEqual(recipeSpec(recipe.schedule!, "Europe/Berlin"), {
    cron: "30 8 * * 1-5 Europe/Berlin"
  });
  const weekly = { ...recipe.schedule!, freq: "weekly", weekday: 5, time: "17:00" } as never;
  assert.deepEqual(recipeSpec(weekly, ""), { cron: "0 17 * * 5" });
});

test("the schedule is always created switched off", () => {
  const input = recipeScheduleInput(recipe, "bot-1", "de", "UTC")!;
  assert.equal(input.enabled, false);
  assert.equal(input.name, "Morning de");
  assert.deepEqual(input.payload, { kind: "agent_prompt", text: "Do it de" });
});

test("apps arrive switched off and only for the new bot; existing apps are untouched", async () => {
  const { api, calls } = fakeApi(["fetch"]);
  const out = await applyRecipeExtras(api, recipe, "bot-1", "en", "UTC");
  assert.deepEqual(out.existing, ["fetch"]);
  assert.deepEqual(out.added, ["brave-search"]);
  assert.equal(calls.servers.length, 1);
  const added = calls.servers[0] as { enabled: boolean; bots: string[]; name: string };
  assert.equal(added.name, "brave-search");
  assert.equal(added.enabled, false);
  assert.deepEqual(added.bots, ["bot-1"]);
  assert.equal(out.scheduleCreated, true);
  assert.equal((calls.schedules[0] as { enabled: boolean }).enabled, false);
});

test("a failing app is reported and does not stop the schedule", async () => {
  const { api } = fakeApi([], "fetch");
  const out = await applyRecipeExtras(api, recipe, "bot-1", "en", "UTC");
  assert.deepEqual(out.failed, ["fetch"]);
  assert.deepEqual(out.added, ["brave-search"]);
  assert.equal(out.scheduleCreated, true);
});
