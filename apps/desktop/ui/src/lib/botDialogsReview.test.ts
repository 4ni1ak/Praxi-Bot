import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { BotSkills } from "../api/types.ts";
import {
  applyCustomText,
  dirtySections,
  editorProblems,
  fallbackList,
  invalidCustomRoles,
  parseFallbackInput,
  addFallback
} from "./botEditor.ts";
import { chatDateLabel } from "./botLanding.ts";
import { configFromDraft, draftFromBot, runtimeTargetLabel } from "./botProfile.ts";
import { createBotSkillsDraft, syncSkillsDraft, toggleDraftSkill } from "./botSkills.ts";
import { isTabStop, trapTarget } from "./focusTrap.ts";
import {
  buildSkillMd,
  checkSkillInstalled,
  emptySkillDraft,
  parseSkillMd,
  type SkillDraft,
  skillDraftDirty
} from "./skillDraft.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");

// --- finding 1 -------------------------------------------------------------

test("configFromDraft keeps the stored runtime verbatim for every target; a new bot gets local", () => {
  for (const computer of ["off", "browser", "host", "auto", "pc", "node:x"]) {
    const bot = { id: "b", name: "A", config: { runtime: { computer, browser: computer, extra: 1 } }, created_at_ms: 1 };
    const cfg = configFromDraft({ ...draftFromBot(bot as never), name: "B" }, bot as never);
    assert.deepEqual((cfg as { runtime: unknown }).runtime, { computer, browser: computer, extra: 1 });
    assert.equal(runtimeTargetLabel(bot.config, "computer"), computer);
  }
  const created = configFromDraft({ ...draftFromBot(undefined), name: "New" }) as { runtime: unknown };
  assert.deepEqual(created.runtime, { computer: "local", browser: "local" });
  assert.equal(runtimeTargetLabel({}, "computer"), "auto");
});

// --- finding 3 -------------------------------------------------------------

test("any custom id can be added as a fallback, in order, several at once", () => {
  assert.deepEqual(parseFallbackInput("custom/odd:model-1"), { refs: ["custom/odd:model-1"] });
  assert.deepEqual(parseFallbackInput("a/b, openrouter/meta/llama-3.3"), {
    refs: ["a/b", "openrouter/meta/llama-3.3"]
  });
  assert.deepEqual(parseFallbackInput(""), { issue: "empty" });
  assert.deepEqual(parseFallbackInput("gemini/"), { issue: "noModel" });
  assert.deepEqual(parseFallbackInput("a b"), { issue: "spaces" });
  const text = ["x/one", "y/two"].reduce(addFallback, "");
  assert.deepEqual(fallbackList(text), ["x/one", "y/two"]);
  // And the field is rendered next to the picker.
  assert.match(read("../components/BotEditorFields.tsx"), /<CustomFallbackEntry/);
});

// --- finding 6 -------------------------------------------------------------

test("an invalid custom model text blocks Save and counts as unsaved data", () => {
  const draft = { ...draftFromBot(undefined), name: "A", executor: { provider: "g", model: "m" } };
  const typed = applyCustomText("gemini/");
  assert.equal(typed.ref, null);
  assert.equal(typed.keep, "gemini/");
  const text = { executor: typed.keep };
  assert.deepEqual(invalidCustomRoles(text), ["executor"]);
  assert.deepEqual(editorProblems(draft, text), [{ problem: "customInvalid", section: "advanced" }]);
  const snap = { draft, initialDraft: draft, apps: {}, initialApps: {}, skillsDirty: false };
  assert.deepEqual(dirtySections({ ...snap, customText: text }), ["advanced"]);
  assert.deepEqual(dirtySections(snap), []);
  // A usable id is stored in the draft, and kept as typed only while it differs from canonical.
  assert.deepEqual(applyCustomText("g/m"), { ref: { provider: "g", model: "m" }, keep: undefined });
  assert.equal(applyCustomText("g/m ").keep, "g/m ");
  // An emptied optional role is a valid edit.
  assert.deepEqual(invalidCustomRoles({ planner: "" }), []);
});

// --- finding 7 -------------------------------------------------------------

const skillsDto = (version: number, names: string[], mode: "all" | "selected" = "selected"): BotSkills =>
  ({
    version,
    mode,
    assigned: mode === "selected" ? names.map((name) => ({ name, state: "ok" })) : [],
    available: ["one", "two", "three"].map((name) => ({ name, description: "", scope: "shared", assigned: names.includes(name) })),
    own: []
  }) as unknown as BotSkills;

test("assigning an added skill keeps the user's pending skill picks", () => {
  const v1 = skillsDto(1, ["one", "two"]);
  let draft = createBotSkillsDraft(v1);
  draft = toggleDraftSkill(draft, "two", false); // pending: drop two
  draft = toggleDraftSkill(draft, "one", true);
  // The add flow assigns "three" on the server from the saved state: [one, two, three].
  const v2 = skillsDto(2, ["one", "two", "three"]);
  const merged = syncSkillsDraft(draft, v2, v1);
  assert.equal(merged.version, 2);
  assert.deepEqual([...merged.selected].sort(), ["one", "three"]);
  // Without pending edits the new assignment simply replaces the picks.
  const clean = syncSkillsDraft(createBotSkillsDraft(v1), v2, v1);
  assert.deepEqual([...clean.selected].sort(), ["one", "three", "two"]);
  // The hook feeds the previous data in.
  assert.match(read("../state/useBotSkills.ts"), /syncSkillsDraft\(cur, data, previous\)/);
});

// --- finding 8 / 9 ----------------------------------------------------------

function draft(over: Partial<SkillDraft> = {}): SkillDraft {
  return { ...emptySkillDraft(), name: "test", description: "Does a thing.", body: "# T\n\nDo it.", ...over };
}

test("'installed' needs the daemon to list exactly this file; a refusal is never hidden", () => {
  const entry = { name: "test", description: "Does a thing.", version: "0.1.0", scope: "shared", requires: null };
  const same = checkSkillInstalled(draft(), { skills: [entry], issues: [] });
  assert.equal(same.state, "installed");
  // The reviewer's case: an older skill of that name AND an issue for skills/test.
  const both = checkSkillInstalled(draft(), {
    skills: [entry],
    issues: [{ path: "C:\\data\\skills\\test\\SKILL.md", message: "`version` must not be empty", scope: "shared" }]
  });
  assert.equal(both.state, "rejected");
  assert.equal(both.message, "`version` must not be empty");
  // Same name, different file (older version, other description, bundled scope).
  for (const other of [{ version: "0.0.9" }, { description: "Other." }, { scope: "bundled" }]) {
    assert.equal(checkSkillInstalled(draft(), { skills: [{ ...entry, ...other }], issues: [] }).state, "different");
  }
  // A bundled-scope issue of the same name is not about our folder.
  assert.equal(
    checkSkillInstalled(draft(), { skills: [], issues: [{ path: "bundled:test", message: "x", scope: "bundled" }] }).state,
    "notFound"
  );
  // Triggers count.
  const withKw = draft({ keywords: "Invoice" });
  assert.equal(checkSkillInstalled(withKw, { skills: [entry], issues: [] }).state, "different");
  assert.equal(
    checkSkillInstalled(withKw, { skills: [{ ...entry, requires: { keywords: ["invoice"] } }], issues: [] }).state,
    "installed"
  );
});

test("pre-check fixtures agree with the daemon's frontmatter rules", () => {
  const file = (front: string) => `---\n${front}\n---\nbody\n`;
  // duplicate top-level / when keys are rejected (frontmatter.rs)
  assert.deepEqual(parseSkillMd(file("name: a\nname: b\ndescription: d")), { ok: false, error: "duplicateKey", detail: "name" });
  assert.deepEqual(
    parseSkillMd(file('name: a\ndescription: d\nwhen:\n  keywords: ["x"]\n  keywords: ["y"]')),
    { ok: false, error: "duplicateKey", detail: "keywords" }
  );
  // an explicit empty version is rejected (skill.rs); an absent one defaults
  assert.deepEqual(parseSkillMd(file('name: a\ndescription: d\nversion: ""')), { ok: false, error: "emptyVersion" });
  const absent = parseSkillMd(file("name: a\ndescription: d"));
  assert.ok(absent.ok && absent.draft.version === "0.1.0");
  // a quoted trigger with a comma round-trips as ONE item
  const parsed = parseSkillMd(file('name: a\ndescription: d\nwhen:\n  keywords: ["hello, world", other]'));
  assert.ok(parsed.ok);
  if (!parsed.ok) return;
  const rebuilt = buildSkillMd(parsed.draft);
  assert.match(rebuilt, /keywords: \["hello, world", "other"\]/);
  const again = parseSkillMd(rebuilt);
  assert.ok(again.ok && again.draft.keywords === parsed.draft.keywords);
});

// --- finding 13 --------------------------------------------------------------

test("the unsaved-skill guard does not depend on the step, only on the file being installed", () => {
  assert.equal(skillDraftDirty({ touched: true, installed: false }), true);
  assert.equal(skillDraftDirty({ touched: true, installed: true }), false);
  assert.equal(skillDraftDirty({ touched: false, installed: false }), false);
  assert.doesNotMatch(read("../components/AddSkillModal.tsx"), /step === "compose" && touched/);
});

// --- finding 10 --------------------------------------------------------------

test("the Tab trap skips inactive tabs and wraps in both directions", () => {
  assert.equal(isTabStop({ tabIndex: -1, rects: 1 }), false);
  assert.equal(isTabStop({ tabIndex: 0, rects: 0 }), false);
  assert.equal(isTabStop({ tabIndex: 0, rects: 2 }), true);
  // Tabs: identity(-1) models(active, 0) advanced(-1); then body input and Save.
  const node = "dialog";
  const items = ["models-tab", "input", "save"];
  assert.equal(trapTarget(items, "models-tab", node, true), "save"); // Shift+Tab from the first stop wraps
  assert.equal(trapTarget(items, "save", node, false), "models-tab"); // forward from the last wraps to the first STOP
  assert.equal(trapTarget(items, "input", node, false), null);
  assert.equal(trapTarget(items, "input", node, true), null);
  // Focus on an inactive tab (clicked): back in, never out.
  assert.equal(trapTarget(items, "identity-tab", node, true), "save");
  assert.equal(trapTarget(items, "identity-tab", node, false), "models-tab");
  assert.equal(trapTarget(items, node, node, true), "save");
  assert.equal(trapTarget([], null, node, false), "none");
  assert.match(read("../components/Modal.tsx"), /trapTarget\(/);
});

// --- finding 5 ---------------------------------------------------------------

test("a chat row shows its creation date, and the panel renders it", () => {
  const label = chatDateLabel(Date.UTC(2026, 10, 15, 12, 30), "en-US");
  assert.match(label, /2026/);
  assert.equal(chatDateLabel(Number.NaN, "en"), "");
  assert.match(read("../components/BotPanel.tsx"), /chatDateLabel\(c\.createdAtMs, i18n\.language\)/);
});

// --- finding 11 --------------------------------------------------------------

test("a failed New chat yields a message, and only the empty state shows it inline", async () => {
  const { createChatFor, newChatFailureNeedsToast } = await import("./newChat.ts");
  let refreshed = 0;
  const refresh = async () => {
    refreshed++;
  };
  const failing = { createChat: async () => Promise.reject(new Error("daemon says no")) };
  const bad = await createChatFor(failing as never, "b1", refresh);
  assert.equal(bad.ok, false);
  // The raw daemon text never reaches the screen (see humanError).
  assert.doesNotMatch(bad.ok ? "" : bad.message, /daemon says no/);
  assert.equal(refreshed, 0);
  const good = await createChatFor({ createChat: async () => ({ id: "c9" }) } as never, "b1", refresh);
  assert.deepEqual(good, { ok: true, chatId: "c9" });
  assert.equal(refreshed, 1);
  // The panel and profile buttons run on a chat route: they need the toast; the empty state not.
  assert.equal(newChatFailureNeedsToast("chat"), true);
  assert.equal(newChatFailureNeedsToast("bot"), false);
  // Both buttons call the one startChat that does this.
  const app = read("../App.tsx");
  assert.match(app, /createChatFor\(client, botId, refreshChats\)/);
  assert.match(app, /onNewChat=\{\(\) => void startChat\(selectedBot\.id\)\}/);
  assert.match(app, /if \(selectedBot\) void startChat\(selectedBot\.id\)/);
});
