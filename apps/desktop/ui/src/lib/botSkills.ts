import type {
  AvailableSkill,
  BotSkills,
  PutBotSkillsInput,
  SkillAssignmentMode
} from "../api/types.ts";
import { matchesQuery } from "./searchText.ts";

/** Local draft state for the "Choose skills" dialog. */
export interface BotSkillsDraft {
  mode: SkillAssignmentMode;
  selected: Set<string>;
  missing: string[];
  initializedSelected: boolean;
  query: string;
}

/** Summary counts rendered on the bot profile card before opening the modal. */
export interface BotSkillsSummary {
  mode: SkillAssignmentMode;
  availableCount: number;
  assignedCount: number;
  missingCount: number;
  ownCount: number;
}

/** Build the initial modal draft from the server's `GET /v1/bots/{id}/skills` payload. */
export function createBotSkillsDraft(dto: BotSkills): BotSkillsDraft {
  if (dto.mode === "selected") {
    return {
      mode: "selected",
      selected: new Set(dto.assigned.map((a) => a.name)),
      missing: dto.assigned
        .filter((a) => a.state === "missing")
        .map((a) => a.name),
      initializedSelected: true,
      query: ""
    };
  }
  return {
    mode: "all",
    selected: new Set(),
    missing: [],
    initializedSelected: false,
    query: ""
  };
}

/**
 * Switch between `"all"` and `"selected"`.
 *
 * The first switch from `"all"` to `"selected"` pre-checks every currently
 * available shared/bundled skill so enabling filtering never silently strips
 * the bot bare. Subsequent switches preserve the existing draft selection.
 */
export function switchSkillMode(
  draft: BotSkillsDraft,
  nextMode: SkillAssignmentMode,
  available: AvailableSkill[]
): BotSkillsDraft {
  if (nextMode === draft.mode) {
    return draft;
  }
  if (nextMode === "selected" && !draft.initializedSelected) {
    return {
      ...draft,
      mode: "selected",
      selected: new Set(available.map((s) => s.name)),
      initializedSelected: true
    };
  }
  return {
    ...draft,
    mode: nextMode
  };
}

/** Toggle one catalog skill inside the draft selection. */
export function toggleDraftSkill(
  draft: BotSkillsDraft,
  name: string,
  checked: boolean
): BotSkillsDraft {
  const selected = new Set(draft.selected);
  if (checked) {
    selected.add(name);
    return { ...draft, selected };
  }
  selected.delete(name);
  return {
    ...draft,
    selected,
    missing: draft.missing.filter((m) => m !== name)
  };
}

/** Remove an unavailable (`missing`) assignment entry from the draft. */
export function removeMissingDraftSkill(
  draft: BotSkillsDraft,
  name: string
): BotSkillsDraft {
  if (!draft.selected.has(name) && !draft.missing.includes(name)) {
    return draft;
  }
  const selected = new Set(draft.selected);
  selected.delete(name);
  return {
    ...draft,
    selected,
    missing: draft.missing.filter((m) => m !== name)
  };
}

/** Filter catalog skills by search query (case- and Turkish-diacritic-insensitive). */
export function filterAvailableSkills(
  available: AvailableSkill[],
  query: string
): AvailableSkill[] {
  return available.filter((s) => matchesQuery(query, s.name, s.description));
}

/** Build the `PUT /v1/bots/{id}/skills` request body from the current draft. */
export function buildPutSkillsPayload(
  draft: BotSkillsDraft,
  available: AvailableSkill[]
): PutBotSkillsInput {
  if (draft.mode === "all") {
    return { mode: "all" };
  }
  const names: string[] = [];
  const seen = new Set<string>();
  for (const s of available) {
    if (draft.selected.has(s.name) && !seen.has(s.name)) {
      seen.add(s.name);
      names.push(s.name);
    }
  }
  for (const m of draft.missing) {
    if (draft.selected.has(m) && !seen.has(m)) {
      seen.add(m);
      names.push(m);
    }
  }
  return { mode: "selected", names };
}

/** Compute non-zero summary numbers for the bot profile skills card. */
export function summarizeBotSkills(dto: BotSkills): BotSkillsSummary {
  const availableCount = dto.available.length;
  const missingCount = dto.assigned.filter((a) => a.state === "missing").length;
  const assignedCount =
    dto.mode === "all"
      ? availableCount
      : dto.assigned.filter((a) => a.state === "ok").length;
  return {
    mode: dto.mode,
    availableCount,
    assignedCount,
    missingCount,
    ownCount: dto.own.length
  };
}

/** Whether the draft differs from the server payload. */
export function isBotSkillsDirty(draft: BotSkillsDraft, dto: BotSkills): boolean {
  if (draft.mode !== dto.mode) {
    return true;
  }
  if (draft.mode === "all") {
    return false;
  }
  const orig = new Set(dto.assigned.map((a) => a.name));
  if (orig.size !== draft.selected.size) {
    return true;
  }
  for (const name of draft.selected) {
    if (!orig.has(name)) {
      return true;
    }
  }
  return false;
}
