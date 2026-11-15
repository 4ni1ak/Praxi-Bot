import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  addFallback,
  BOT_EDITOR_SECTIONS,
  closeAction,
  customRef,
  customRefIssue,
  dirtySections,
  draftsEqual,
  editorProblems,
  fallbackList,
  isEditorDirty,
  permissionTone,
  removeFallback,
  stepSection
} from "./botEditor.ts";
import { type BotDraft, draftFromBot } from "./botProfile.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");

function base(): BotDraft {
  return {
    ...draftFromBot(undefined),
    name: "Atlas",
    executor: { provider: "gemini", model: "gemini-3.5-flash-lite" }
  };
}

const snap = (draft: BotDraft, over: Partial<Parameters<typeof dirtySections>[0]> = {}) => ({
  draft,
  initialDraft: base(),
  apps: { github: true },
  initialApps: { github: true },
  skillsDirty: false,
  ...over
});

test("an untouched draft is clean", () => {
  assert.equal(draftsEqual(base(), base()), true);
  assert.equal(isEditorDirty(snap(base())), false);
  // Cosmetic differences do not count as edits.
  assert.equal(isEditorDirty(snap({ ...base(), name: "Atlas  " })), false);
  assert.equal(isEditorDirty(snap({ ...base(), color: base().color.toUpperCase() })), false);
  assert.equal(isEditorDirty(snap({ ...base(), fallback: " " })), false);
});

test("each kind of edit marks its own section", () => {
  assert.deepEqual(dirtySections(snap({ ...base(), persona: "Careful." })), ["identity"]);
  assert.deepEqual(
    dirtySections(snap({ ...base(), planner: { provider: "openai", model: "gpt-5" } })),
    ["models"]
  );
  assert.deepEqual(dirtySections(snap({ ...base(), fallback: "openai/gpt-5" })), ["models"]);
  assert.deepEqual(dirtySections(snap({ ...base(), preset: "full" })), ["permissions"]);
  const cats = { ...base().categories, shell: "deny" as const };
  assert.deepEqual(dirtySections(snap({ ...base(), categories: cats })), ["permissions"]);
  assert.deepEqual(dirtySections(snap(base(), { skillsDirty: true })), ["skills"]);
  assert.deepEqual(dirtySections(snap(base(), { apps: { github: false } })), ["apps"]);
  // An app that appeared unticked is not a change.
  assert.deepEqual(dirtySections(snap(base(), { apps: { github: true, slack: false } })), []);
  assert.equal(draftsEqual(base(), { ...base(), avatar: "data:image/png;base64,AA" }), false);
});

test("save is blocked by a missing name or executor, pointing at the right section", () => {
  assert.deepEqual(editorProblems(base()), []);
  assert.deepEqual(editorProblems({ ...base(), name: "  " }), [
    { problem: "nameMissing", section: "identity" }
  ]);
  assert.deepEqual(
    editorProblems({ ...base(), name: "", executor: { provider: "gemini", model: "" } }),
    [
      { problem: "nameMissing", section: "identity" },
      { problem: "executorMissing", section: "models" }
    ]
  );
});

test("closing asks first when dirty and waits while saving", () => {
  assert.equal(closeAction(false, false), "close");
  assert.equal(closeAction(true, false), "confirm");
  assert.equal(closeAction(true, true), "wait");
  assert.equal(closeAction(false, true), "wait");
});

test("section keyboard navigation wraps and ignores other keys", () => {
  assert.equal(stepSection("identity", "ArrowDown"), "models");
  assert.equal(stepSection("identity", "ArrowUp"), "advanced");
  assert.equal(stepSection("advanced", "ArrowRight"), "identity");
  assert.equal(stepSection("skills", "Home"), "identity");
  assert.equal(stepSection("skills", "End"), "advanced");
  assert.equal(stepSection("skills", "Enter"), null);
  assert.equal(new Set(BOT_EDITOR_SECTIONS).size, BOT_EDITOR_SECTIONS.length);
});

test("fallback models: ordered, de-duplicated, add and remove", () => {
  assert.deepEqual(fallbackList(" a/x, b/y ,, a/x "), ["a/x", "b/y"]);
  assert.equal(addFallback("a/x", "b/y"), "a/x, b/y");
  assert.equal(addFallback("a/x", "a/x"), "a/x");
  assert.equal(addFallback("", "a/x"), "a/x");
  assert.equal(removeFallback("a/x, b/y", "a/x"), "b/y");
  assert.equal(removeFallback("a/x", "a/x"), "");
});

test("custom model ids are checked before they reach the draft", () => {
  assert.equal(customRefIssue(""), "empty");
  assert.equal(customRefIssue("gemini/ gemini 3"), "spaces");
  assert.equal(customRefIssue("gemini/"), "noModel");
  assert.equal(customRefIssue("gemini/gemini-3.5-flash-lite"), null);
  assert.equal(customRefIssue("my-model"), null);
  assert.deepEqual(customRef(" openrouter/meta-llama/llama-3.3 "), {
    provider: "openrouter",
    model: "meta-llama/llama-3.3"
  });
  assert.deepEqual(customRef("my-model"), { provider: "", model: "my-model" });
  assert.equal(customRef("gemini/"), null);
});

test("permission chip tone follows what runs without asking", () => {
  assert.equal(permissionTone("read-only"), "locked");
  assert.equal(permissionTone("approve-destructive"), "guarded");
  assert.equal(permissionTone("read-and-draft"), "guarded");
  assert.equal(permissionTone("full"), "open");
  assert.equal(permissionTone("custom"), "custom");
});

// Wiring: the dialog must decide through these functions, and must not fall back to native
// selects or a free-text model box outside the Advanced section.
test("BotEditModal is driven by the tested rules", () => {
  const modal = read("../views/BotEditModal.tsx");
  for (const fn of ["closeAction(", "dirtySections(", "editorProblems(", "stepSection("]) {
    assert.ok(modal.includes(fn), `BotEditModal uses ${fn}`);
  }
  assert.doesNotMatch(modal, /<select/);
  const fields = read("../components/BotEditorFields.tsx");
  for (const fn of ["addFallback(", "removeFallback(", "customRefIssue(", "fallbackList("]) {
    assert.ok(fields.includes(fn), `BotEditorFields uses ${fn}`);
  }
  assert.doesNotMatch(fields, /<select/);
  assert.match(fields, /<ModelPicker/);
});
