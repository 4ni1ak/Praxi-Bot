import type { ApiClient } from "../api/client.ts";
import type { Bot } from "../api/types.ts";
import { type BotDraft, configFromDraft, draftFromBot } from "./botProfile.ts";
import { type AppsResult, applyAppsSelection } from "./mcpApps.ts";

/**
 * The combined Save of the "Edit bot" dialog (profile, app ticks, skills) as one controller, so
 * the component only renders its outcome.  Rules (review of PR #95, findings 1 and 2):
 *
 * - the profile is written only when a profile field was edited - a skills-only or apps-only Save
 *   never PUTs the bot;
 * - what is written is the *fresh* stored profile with only the fields the user changed laid over
 *   it, so unedited fields (including `runtime`, i.e. the computer target, and unknown keys)
 *   round-trip untouched;
 * - the daemon has no profile revision or ETag (`PUT /v1/bots/{id}` replaces unconditionally), so
 *   the conflict check compares the stored profile with the one the dialog was opened on, right
 *   before writing.  It cannot be atomic - the window is one request - which is the best the
 *   current API allows.
 */

type Json = Record<string, unknown>;

/** JSON with sorted object keys, so equal profiles compare equal whatever the key order. */
export function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value && typeof value === "object") {
    const o = value as Json;
    return `{${Object.keys(o)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${stableJson(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

/** Whether two stored bots hold the same name and profile. */
export function sameProfile(a: Pick<Bot, "name" | "config">, b: Pick<Bot, "name" | "config">): boolean {
  return a.name === b.name && stableJson(a.config ?? {}) === stableJson(b.config ?? {});
}

export type ProfilePart = "identity" | "models" | "permissions" | "runtime" | "other";

/** What another client changed in the stored profile since `base` (for the conflict notice). */
export function changedProfileParts(
  base: Pick<Bot, "name" | "config">,
  fresh: Pick<Bot, "name" | "config">
): ProfilePart[] {
  const a = (base.config ?? {}) as Json;
  const b = (fresh.config ?? {}) as Json;
  const differs = (x: unknown, y: unknown) => stableJson(x ?? null) !== stableJson(y ?? null);
  const out: ProfilePart[] = [];
  if (base.name !== fresh.name || differs(a.bot, b.bot)) out.push("identity");
  if (differs(a.models, b.models)) out.push("models");
  if (differs(a.permissions, b.permissions)) out.push("permissions");
  if (differs(a.runtime, b.runtime)) out.push("runtime");
  const known = new Set(["bot", "models", "permissions", "runtime"]);
  const rest = (c: Json) => Object.fromEntries(Object.entries(c).filter(([k]) => !known.has(k)));
  if (differs(rest(a), rest(b))) out.push("other");
  return out;
}

const DRAFT_KEYS: ReadonlyArray<ReadonlyArray<keyof BotDraft>> = [
  ["name"],
  ["persona"],
  ["shape"],
  ["color"],
  ["avatar"],
  ["executor"],
  ["planner"],
  ["classifier"],
  ["fallback"],
  // The preset and the per-category levels are one setting.
  ["preset", "categories"]
];

/**
 * The draft to save: the stored (fresh) values, with every field the user edited (`mine` against
 * `opened`) taken from `mine`.
 */
export function mergeDraft(fresh: BotDraft, mine: BotDraft, opened: BotDraft): BotDraft {
  const out: BotDraft = { ...fresh };
  for (const group of DRAFT_KEYS) {
    const edited = group.some((k) => stableJson(mine[k]) !== stableJson(opened[k]));
    if (!edited) continue;
    for (const k of group) (out as unknown as Json)[k] = mine[k];
  }
  return out;
}

export interface EditorSaveArgs {
  /** The stored bot the dialog was opened on (or last saved). */
  baseline: Bot;
  draft: BotDraft;
  /** The draft the dialog was opened on (or last saved). */
  openedDraft: BotDraft;
  /** Whether any identity / model / permission field was edited. */
  profileDirty: boolean;
  initialApps: Record<string, boolean>;
  allowed: Record<string, boolean>;
  allBotIds: string[];
  /** The user chose "Overwrite" after a conflict: write over whatever is stored now. */
  overwrite?: boolean;
  /** Saves the skills assignment (already validated), when it was edited. */
  saveSkills?: (() => Promise<SkillsOutcome>) | null;
}

export type SkillsOutcome =
  | { ok: true }
  | { ok: false; failure: "conflict" | "invalid" | "error"; message: string };

export type EditorSaveResult =
  | { kind: "conflict"; fresh: Bot; changed: ProfilePart[] }
  | ({
      kind: "saved";
      /** The stored bot afterwards (the written one, or the untouched baseline). */
      bot: Bot;
      profileWritten: boolean;
      skills: SkillsOutcome | null;
    } & AppsResult);

export async function saveEditor(
  api: Pick<ApiClient, "getBot" | "updateBot" | "listMcpServers" | "updateMcpServer">,
  a: EditorSaveArgs
): Promise<EditorSaveResult> {
  let bot = a.baseline;
  let profileWritten = false;
  if (a.profileDirty) {
    const fresh = await api.getBot(a.baseline.id);
    if (!a.overwrite && !sameProfile(fresh, a.baseline)) {
      return { kind: "conflict", fresh, changed: changedProfileParts(a.baseline, fresh) };
    }
    const merged = mergeDraft(draftFromBot(fresh), a.draft, a.openedDraft);
    bot = await api.updateBot(fresh.id, {
      name: merged.name.trim(),
      config: configFromDraft(merged, fresh)
    });
    profileWritten = true;
  }
  const apps = await applyAppsSelection(api, bot.id, a.initialApps, a.allowed, a.allBotIds);
  const skills = a.saveSkills ? await a.saveSkills() : null;
  return { kind: "saved", bot, profileWritten, skills, ...apps };
}
