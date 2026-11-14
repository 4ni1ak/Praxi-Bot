import assert from "node:assert/strict";
import { test } from "node:test";
import { MockApiClient } from "../api/mock.ts";
import type { BotSkills } from "../api/types.ts";
import {
  buildPutSkillsPayload,
  createBotSkillsDraft,
  filterAvailableSkills,
  isBotSkillsDirty,
  removeMissingDraftSkill,
  summarizeBotSkills,
  switchSkillMode,
  toggleDraftSkill
} from "./botSkills.ts";

const SAMPLE_BOT_SKILLS: BotSkills = {
  mode: "all",
  assigned: [],
  available: [
    {
      name: "code-review",
      description: "Inspect pull requests for bugs.",
      version: "1.2.0",
      scope: "bundled",
      requires: { apps: [], domains: [], keywords: [] },
      hub: null,
      assigned: true
    },
    {
      name: "github-triage",
      description: "Categorize issues and check duplicates.",
      version: "1.0.0",
      scope: "shared",
      requires: { apps: ["github"], domains: ["github.com"], keywords: [] },
      hub: { source: "clawhub", locked: true },
      assigned: true
    }
  ],
  own: [{ name: "atlas-standup", version: "0.1.0" }]
};

test("first switch from all to selected pre-checks every currently visible skill", () => {
  let draft = createBotSkillsDraft(SAMPLE_BOT_SKILLS);
  assert.equal(draft.mode, "all");
  assert.equal(draft.initializedSelected, false);
  assert.equal(isBotSkillsDirty(draft, SAMPLE_BOT_SKILLS), false);

  draft = switchSkillMode(draft, "selected", SAMPLE_BOT_SKILLS.available);
  assert.equal(draft.mode, "selected");
  assert.equal(draft.initializedSelected, true);
  assert.deepEqual([...draft.selected].sort(), ["code-review", "github-triage"]);
  assert.equal(isBotSkillsDirty(draft, SAMPLE_BOT_SKILLS), true);

  // Uncheck one, toggle back to "all", then back to "selected": edits stay intact.
  draft = toggleDraftSkill(draft, "github-triage", false);
  assert.deepEqual([...draft.selected], ["code-review"]);

  draft = switchSkillMode(draft, "all", SAMPLE_BOT_SKILLS.available);
  assert.equal(draft.mode, "all");
  assert.deepEqual(buildPutSkillsPayload(draft, SAMPLE_BOT_SKILLS.available), {
    mode: "all"
  });

  draft = switchSkillMode(draft, "selected", SAMPLE_BOT_SKILLS.available);
  assert.deepEqual([...draft.selected], ["code-review"]);
  assert.deepEqual(buildPutSkillsPayload(draft, SAMPLE_BOT_SKILLS.available), {
    mode: "selected",
    names: ["code-review"]
  });
});

test("selected mode preserves missing entries until explicitly removed", () => {
  const dto: BotSkills = {
    mode: "selected",
    assigned: [
      { name: "code-review", state: "ok" },
      { name: "gone-skill", state: "missing" }
    ],
    available: SAMPLE_BOT_SKILLS.available,
    own: []
  };

  let draft = createBotSkillsDraft(dto);
  assert.deepEqual(draft.missing, ["gone-skill"]);
  assert.equal(isBotSkillsDirty(draft, dto), false);
  assert.deepEqual(buildPutSkillsPayload(draft, dto.available), {
    mode: "selected",
    names: ["code-review", "gone-skill"]
  });

  draft = removeMissingDraftSkill(draft, "gone-skill");
  assert.deepEqual(draft.missing, []);
  assert.equal(isBotSkillsDirty(draft, dto), true);
  assert.deepEqual(buildPutSkillsPayload(draft, dto.available), {
    mode: "selected",
    names: ["code-review"]
  });
});

test("filterAvailableSkills matches Turkish diacritics and summarizeBotSkills counts missing separately", () => {
  const withTurkish: BotSkills = {
    mode: "selected",
    assigned: [
      { name: "gunluk-ozet", state: "ok" },
      { name: "vanished", state: "missing" }
    ],
    available: [
      {
        name: "gunluk-ozet",
        description: "Günlük çalışma özetini çıkarır",
        version: "0.1.0",
        scope: "shared",
        requires: { apps: [], domains: [], keywords: [] },
        hub: null,
        assigned: true
      }
    ],
    own: [{ name: "learned-one", version: "" }]
  };

  assert.equal(
    filterAvailableSkills(withTurkish.available, "GUNLUK ozet").length,
    1
  );
  assert.equal(filterAvailableSkills(withTurkish.available, "nope").length, 0);

  assert.deepEqual(summarizeBotSkills(withTurkish), {
    mode: "selected",
    availableCount: 1,
    assignedCount: 1,
    missingCount: 1,
    ownCount: 1
  });
  assert.deepEqual(summarizeBotSkills(SAMPLE_BOT_SKILLS), {
    mode: "all",
    availableCount: 2,
    assignedCount: 2,
    missingCount: 0,
    ownCount: 1
  });
});

test("MockApiClient getBotSkills and putBotSkills round-trip and validate", async () => {
  const client = new MockApiClient();
  const bots = await client.listBots();
  const atlas = bots[0];
  assert.ok(atlas);

  const catalog = await client.listSkills();
  assert.ok(catalog.skills.length >= 2);

  const initial = await client.getBotSkills(atlas.id);
  assert.equal(initial.mode, "all");
  assert.ok(initial.own.length >= 1);

  const updated = await client.putBotSkills(atlas.id, {
    mode: "selected",
    names: [catalog.skills[0].name]
  });
  assert.equal(updated.mode, "selected");
  assert.deepEqual(updated.assigned, [{ name: catalog.skills[0].name, state: "ok" }]);

  await assert.rejects(() =>
    client.putBotSkills(atlas.id, { mode: "selected", names: ["Invalid Name"] })
  );

  const backToAll = await client.putBotSkills(atlas.id, { mode: "all" });
  assert.equal(backToAll.mode, "all");
});
