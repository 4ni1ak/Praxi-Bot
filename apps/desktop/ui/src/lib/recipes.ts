import type { components } from "../api/schema";
import type { McpCatalogEntry, McpServer, ScheduleInput } from "../api/types";
import { configFromCatalog } from "./mcpApps.ts";

type S = components["schemas"];
export type Recipe = S["Recipe"];
export type RecipeApp = S["RecipeApp"];
export type RecipeSchedule = S["RecipeSchedule"];

/** The text in `lang`, falling back to English (a missing language never hides a recipe). */
export function recipeText(map: Record<string, string>, lang: string): string {
  const short = lang.toLowerCase().split("-")[0] ?? "en";
  return map[short] ?? map.en ?? "";
}

export function recipeLines(map: Record<string, string[]>, lang: string): string[] {
  const short = lang.toLowerCase().split("-")[0] ?? "en";
  return map[short] ?? map.en ?? [];
}

/** The daemon `spec` of a recipe schedule: a cron line in the person's own time zone. */
export function recipeSpec(s: RecipeSchedule, tz: string): { cron: string } {
  const [h, m] = s.time.split(":").map((n) => Number.parseInt(n, 10));
  const day = s.freq === "weekdays" ? "1-5" : s.freq === "weekly" ? String(s.weekday ?? 5) : "*";
  const expr = `${m} ${h} * * ${day}`;
  return { cron: tz.trim() ? `${expr} ${tz.trim()}` : expr };
}

/**
 * The schedule of a recipe as a daemon request. It is always created switched OFF: the recipe
 * only prepares it, the person turns it on (and sees it in Schedules).
 */
export function recipeScheduleInput(
  recipe: Recipe,
  botId: string,
  lang: string,
  tz: string
): ScheduleInput | null {
  const s = recipe.schedule;
  if (!s) return null;
  return {
    bot_id: botId,
    name: recipeText(s.title, lang),
    spec: recipeSpec(s, tz),
    payload: { kind: "agent_prompt", text: recipeText(s.prompt, lang) },
    enabled: false
  };
}

/** The server a recipe app adds: from the catalog entry, switched OFF and only for the new bot. */
export function recipeAppConfig(entry: McpCatalogEntry, botId: string) {
  return { ...configFromCatalog(entry), enabled: false, bots: [botId] };
}

export interface RecipeApplyApi {
  listMcpServers(): Promise<McpServer[]>;
  mcpCatalog(): Promise<McpCatalogEntry[]>;
  createMcpServer(config: ReturnType<typeof recipeAppConfig>): Promise<unknown>;
  createSchedule(input: ScheduleInput): Promise<unknown>;
}

export interface RecipeApplyResult {
  /** Apps added switched off. */
  added: string[];
  /** Apps that were already there: left exactly as they are. */
  existing: string[];
  /** Apps or the schedule that could not be prepared. */
  failed: string[];
  scheduleCreated: boolean;
}

/**
 * After the bot exists: add the apps (switched off, for this bot only) and the schedule
 * (switched off). Nothing here connects anything, stores a secret or changes an app the person
 * already has; each step fails on its own and is reported, never retried silently.
 */
export async function applyRecipeExtras(
  api: RecipeApplyApi,
  recipe: Recipe,
  botId: string,
  lang: string,
  tz: string
): Promise<RecipeApplyResult> {
  const out: RecipeApplyResult = { added: [], existing: [], failed: [], scheduleCreated: false };
  if (recipe.apps && recipe.apps.length > 0) {
    try {
      const [catalog, servers] = await Promise.all([api.mcpCatalog(), api.listMcpServers()]);
      const have = new Set(servers.map((s) => s.name));
      for (const app of recipe.apps) {
        const entry = catalog.find((e) => e.id === app.catalog_id);
        if (!entry) {
          out.failed.push(app.catalog_id);
        } else if (have.has(entry.id)) {
          out.existing.push(entry.id);
        } else {
          try {
            await api.createMcpServer(recipeAppConfig(entry, botId));
            out.added.push(entry.id);
          } catch {
            out.failed.push(entry.id);
          }
        }
      }
    } catch {
      out.failed.push(...recipe.apps.map((a) => a.catalog_id));
    }
  }
  const sched = recipeScheduleInput(recipe, botId, lang, tz);
  if (sched) {
    try {
      await api.createSchedule(sched);
      out.scheduleCreated = true;
    } catch {
      out.failed.push("schedule");
    }
  }
  return out;
}
