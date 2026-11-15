import { PERMISSION_CATEGORIES } from "../api/types.ts";
import {
  type BotDraft,
  formatModelRef,
  type ModelRef,
  parseModelRef,
  type PresetChoice
} from "./botProfile.ts";

/**
 * State rules of the "Edit bot" dialog (owner feedback 244, point 5): its sections, what counts
 * as unsaved, what blocks saving, and what closing does.  The dialog component only renders.
 */

export const BOT_EDITOR_SECTIONS = [
  "identity",
  "models",
  "permissions",
  "skills",
  "apps",
  "advanced"
] as const;

export type BotEditorSection = (typeof BOT_EDITOR_SECTIONS)[number];

/** Arrow / Home / End navigation of the section list (wraps around). */
export function stepSection(
  current: BotEditorSection,
  key: string
): BotEditorSection | null {
  const i = BOT_EDITOR_SECTIONS.indexOf(current);
  const n = BOT_EDITOR_SECTIONS.length;
  const at = (j: number): BotEditorSection => BOT_EDITOR_SECTIONS[(j + n) % n] ?? current;
  switch (key) {
    case "ArrowDown":
    case "ArrowRight":
      return at(i + 1);
    case "ArrowUp":
    case "ArrowLeft":
      return at(i - 1);
    case "Home":
      return at(0);
    case "End":
      return at(n - 1);
    default:
      return null;
  }
}

function sameRef(a: ModelRef, b: ModelRef): boolean {
  return formatModelRef(a) === formatModelRef(b);
}

/** Field-by-field comparison of two drafts (trailing spaces in the name do not count). */
export function draftsEqual(a: BotDraft, b: BotDraft): boolean {
  return (
    a.name.trim() === b.name.trim() &&
    a.persona === b.persona &&
    a.shape === b.shape &&
    a.color.toLowerCase() === b.color.toLowerCase() &&
    (a.avatar ?? null) === (b.avatar ?? null) &&
    sameRef(a.executor, b.executor) &&
    sameRef(a.planner, b.planner) &&
    sameRef(a.classifier, b.classifier) &&
    fallbackList(a.fallback).join(",") === fallbackList(b.fallback).join(",") &&
    a.preset === b.preset &&
    PERMISSION_CATEGORIES.every((c) => a.categories[c] === b.categories[c]) &&
    a.computer === b.computer &&
    a.browser === b.browser
  );
}

export interface EditorSnapshot {
  draft: BotDraft;
  initialDraft: BotDraft;
  apps: Record<string, boolean>;
  initialApps: Record<string, boolean>;
  skillsDirty: boolean;
}

function appsChanged(a: Record<string, boolean>, b: Record<string, boolean>): boolean {
  const names = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const name of names) {
    if ((a[name] ?? false) !== (b[name] ?? false)) return true;
  }
  return false;
}

/** Which sections hold unsaved changes. */
export function dirtySections(s: EditorSnapshot): BotEditorSection[] {
  const { draft: d, initialDraft: i } = s;
  const out: BotEditorSection[] = [];
  if (
    d.name.trim() !== i.name.trim() ||
    d.persona !== i.persona ||
    d.shape !== i.shape ||
    d.color.toLowerCase() !== i.color.toLowerCase() ||
    (d.avatar ?? null) !== (i.avatar ?? null)
  ) {
    out.push("identity");
  }
  if (
    !sameRef(d.executor, i.executor) ||
    !sameRef(d.planner, i.planner) ||
    !sameRef(d.classifier, i.classifier) ||
    fallbackList(d.fallback).join(",") !== fallbackList(i.fallback).join(",")
  ) {
    out.push("models");
  }
  if (d.preset !== i.preset || PERMISSION_CATEGORIES.some((c) => d.categories[c] !== i.categories[c])) {
    out.push("permissions");
  }
  if (s.skillsDirty) out.push("skills");
  if (appsChanged(s.apps, s.initialApps)) out.push("apps");
  if (d.computer !== i.computer || d.browser !== i.browser) out.push("advanced");
  return out;
}

export function isEditorDirty(s: EditorSnapshot): boolean {
  return dirtySections(s).length > 0;
}

export type EditorProblem = "nameMissing" | "executorMissing";

/** What blocks Save, and the section that fixes it. */
export function editorProblems(
  draft: BotDraft
): Array<{ problem: EditorProblem; section: BotEditorSection }> {
  const out: Array<{ problem: EditorProblem; section: BotEditorSection }> = [];
  if (draft.name.trim() === "") out.push({ problem: "nameMissing", section: "identity" });
  if (draft.executor.model.trim() === "") {
    out.push({ problem: "executorMissing", section: "models" });
  }
  return out;
}

/**
 * Closing the dialog (Esc, backdrop, the X, Cancel): never while a save is in flight, and only
 * after a confirmation when something is unsaved.
 */
export function closeAction(dirty: boolean, saving: boolean): "close" | "confirm" | "wait" {
  if (saving) return "wait";
  return dirty ? "confirm" : "close";
}

// --- fallback models -------------------------------------------------------

/** The stored comma list as ordered, de-duplicated refs. */
export function fallbackList(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of text.split(",")) {
    const ref = part.trim();
    if (ref && !seen.has(ref)) {
      seen.add(ref);
      out.push(ref);
    }
  }
  return out;
}

export function addFallback(text: string, ref: string): string {
  return fallbackList(`${text},${ref}`).join(", ");
}

export function removeFallback(text: string, ref: string): string {
  return fallbackList(text)
    .filter((r) => r !== ref)
    .join(", ");
}

// --- custom model id (the "Advanced" escape hatch) -------------------------

export type CustomRefIssue = "empty" | "spaces" | "noModel";

/**
 * Check a hand-typed `provider/model` id.  The model part may itself contain slashes
 * (`openrouter/meta-llama/llama-3.3`); a missing provider is allowed (default provider).
 */
export function customRefIssue(text: string): CustomRefIssue | null {
  const t = text.trim();
  if (!t) return "empty";
  if (/\s/.test(t)) return "spaces";
  const ref = parseModelRef(t);
  if (!ref.model.trim()) return "noModel";
  return null;
}

/** A typed id as the `ModelRef` a draft stores; `null` while it is not usable. */
export function customRef(text: string): ModelRef | null {
  return customRefIssue(text) ? null : parseModelRef(text.trim());
}

// --- permission mode chip --------------------------------------------------

export type PermissionTone = "locked" | "guarded" | "open" | "custom";

/** How the permission chip is tinted: what the bot may do without asking. */
export function permissionTone(preset: PresetChoice): PermissionTone {
  switch (preset) {
    case "read-only":
      return "locked";
    case "full":
      return "open";
    case "custom":
      return "custom";
    case "approve-destructive":
    case "read-and-draft":
      return "guarded";
  }
}
