import assert from "node:assert/strict";
import { test } from "node:test";
import { MockApiClient } from "./mock.ts";

test("MockApiClient serves workspace overview, file browsing, rename, delete+undo, limits, and cleanup", async () => {
  const client = new MockApiClient();
  const initial = await client.listWorkspaces();
  assert.equal(initial.configured, true);
  assert.ok(initial.bots.length >= 1);
  assert.equal(initial.settings.warn_percent_1, 80);
  assert.equal(initial.settings.warn_percent_2, 90);
  assert.equal(initial.settings.warn_percent_3, 95);

  const atlasId = initial.bots[0]!.bot_id;
  const rootList = await client.getBotWorkspace(atlasId);
  const dirs = rootList.entries.filter((e) => e.kind === "directory").map((e) => e.name);
  for (const sub of ["leads", "drafts", "downloads", "notes", "logs"]) {
    assert.ok(dirs.includes(sub), `missing ${sub}`);
  }

  const hits = await client.getBotWorkspace(atlasId, { q: "weekly" });
  assert.equal(hits.entries.length, 1);
  assert.equal(hits.entries[0]!.path, "notes/weekly-summary.md");

  const blob = await client.getBotWorkspaceFile(atlasId, "notes/weekly-summary.md", false);
  assert.ok((await blob.text()).includes("Weekly Summary"));

  const renamed = await client.renameBotWorkspaceEntry(atlasId, {
    path: "notes/weekly-summary.md",
    new_name: "q4-summary.md"
  });
  assert.equal(renamed.path, "notes/q4-summary.md");

  const deleted = await client.deleteBotWorkspaceEntry(atlasId, { path: "notes/q4-summary.md" });
  const restored = await client.restoreBotWorkspaceEntry(atlasId, { undo_id: deleted.undo_id });
  assert.equal(restored.path, "notes/q4-summary.md");

  const limited = await client.putBotWorkspaceLimits(atlasId, {
    soft_limit_bytes: 1024,
    hard_limit_bytes: 4096
  });
  assert.equal(limited.soft_limit_bytes, 1024);
  assert.equal(limited.hard_limit_bytes, 4096);

  const updatedSettings = await client.updateWorkspaceSettings({
    warn_percent_1: 70,
    warn_percent_2: 85,
    warn_percent_3: 95,
    min_free_bytes: 5 * 1024 ** 3,
    downloads_max_age_days: 14,
    logs_max_age_days: 14
  });
  assert.equal(updatedSettings.warn_percent_1, 70);

  const cleaned = await client.runWorkspaceCleanup({ target: "old_downloads" });
  assert.ok(cleaned.deleted_files >= 1);
  assert.ok(cleaned.freed_bytes > 0);
});
