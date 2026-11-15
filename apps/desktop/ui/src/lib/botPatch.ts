import type { Bot, PermissionCategoryName } from "../api/types.ts";
import { PERMISSION_CATEGORIES } from "../api/types.ts";
import { fallbackList } from "./botEditor.ts";
import { type BotDraft, formatModelRef, type ModelRef } from "./botProfile.ts";

/**
 * Field-level bot edits for `PATCH /v1/bots/{id}`: the ops carry ONLY what the user changed
 * (the dialog's draft against the draft it opened with), so a save can never rewrite a field the
 * user did not touch - the daemon applies them atomically against `expected_version`.
 */

export type PatchOp =
  | { op: "set"; path: string; value: unknown }
  | { op: "unset"; path: string };

export interface ProfilePatch {
  name?: string;
  ops: PatchOp[];
}

const set = (path: string, value: unknown): PatchOp => ({ op: "set", path, value });
const unset = (path: string): PatchOp => ({ op: "unset", path });

function sameRef(a: ModelRef, b: ModelRef): boolean {
  return formatModelRef(a) === formatModelRef(b);
}

/** What changed between the draft the dialog opened with and the draft now. */
export function profilePatch(opened: BotDraft, draft: BotDraft): ProfilePatch {
  const ops: PatchOp[] = [];
  const patch: ProfilePatch = { ops };

  const name = draft.name.trim();
  if (name !== opened.name.trim()) {
    patch.name = name;
    ops.push(set("/bot/name", name));
  }
  if (draft.persona !== opened.persona) ops.push(set("/bot/persona", draft.persona));
  if (draft.shape !== opened.shape) ops.push(set("/bot/shape", draft.shape));
  if (draft.color.toLowerCase() !== opened.color.toLowerCase()) {
    ops.push(set("/bot/color", draft.color));
  }
  if ((draft.avatar ?? null) !== (opened.avatar ?? null)) {
    ops.push(draft.avatar ? set("/bot/avatar", draft.avatar) : unset("/bot/avatar"));
  }

  if (!sameRef(draft.executor, opened.executor)) {
    ops.push(set("/models/executor", formatModelRef(draft.executor)));
  }
  for (const role of ["planner", "classifier"] as const) {
    if (sameRef(draft[role], opened[role])) continue;
    const ref = formatModelRef(draft[role]);
    ops.push(ref ? set(`/models/${role}`, ref) : unset(`/models/${role}`));
  }
  const before = fallbackList(opened.fallback);
  const after = fallbackList(draft.fallback);
  if (before.join("\n") !== after.join("\n")) {
    ops.push(after.length > 0 ? set("/models/fallback", after) : unset("/models/fallback"));
  }

  if (draft.preset !== opened.preset && draft.preset !== "custom") {
    // An explicit preset choice replaces the whole policy (as the old page did).
    ops.push(set("/permissions", { preset: draft.preset }));
  } else {
    // One rule per category: an edited category rewrites only its `level`, so its limits and
    // other settings stay, and every other category is not mentioned at all.
    for (const c of PERMISSION_CATEGORIES) {
      if (draft.categories[c] !== opened.categories[c]) {
        ops.push(set(`/permissions/categories/${c}/level`, { level: draft.categories[c] }));
      }
    }
  }
  return patch;
}

export function isEmptyPatch(p: ProfilePatch): boolean {
  return p.name === undefined && p.ops.length === 0;
}

// --- the daemon's op semantics, for the mock client and the tests ---------------------------

type Json = Record<string, unknown>;

function patchAt(node: unknown, keys: string[], value: { v: unknown } | null): void {
  const [first, ...rest] = keys;
  if (first === undefined) return;
  if (!node || typeof node !== "object" || Array.isArray(node)) {
    throw new Error("path crosses a non-object");
  }
  const obj = node as Json;
  if (rest.length === 0) {
    if (value) obj[first] = structuredClone(value.v);
    else delete obj[first];
    return;
  }
  if (!(first in obj)) {
    if (!value) return;
    obj[first] = {};
  }
  patchAt(obj[first], rest, value);
}

/** The daemon's `apply_patch_ops` (JSON Pointer over object keys; set creates parents). */
export function applyPatchOps(config: unknown, ops: readonly PatchOp[]): Json {
  const next = structuredClone((config ?? {}) as Json);
  for (const op of ops) {
    if (!op.path.startsWith("/")) throw new Error(`path ${op.path} must start with /`);
    const keys = op.path
      .slice(1)
      .split("/")
      .map((k) => k.replace(/~1/g, "/").replace(/~0/g, "~"));
    if (keys.some((k) => k === "")) throw new Error(`path ${op.path} has an empty segment`);
    patchAt(next, keys, op.op === "set" ? { v: op.value } : null);
  }
  return next;
}

/** The categories a patch mentions (tests and the conflict notice). */
export function patchedCategories(p: ProfilePatch): PermissionCategoryName[] {
  return PERMISSION_CATEGORIES.filter((c) =>
    p.ops.some((o) => o.path.startsWith(`/permissions/categories/${c}/`))
  );
}

/** The bot a conflict (`409`) carried, if the error is one. */
export function conflictBot(e: unknown): Bot | null {
  const err = e as { status?: number; code?: string; body?: unknown } | null;
  if (!err || err.status !== 409 || err.code !== "version_conflict") return null;
  const cur = (err.body as { current?: Bot } | undefined)?.current;
  return cur ?? null;
}
