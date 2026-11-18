import assert from "node:assert/strict";
import { test } from "node:test";
import {
  defaultKnowledgeSourceLabel,
  MAX_KNOWLEDGE_SOURCE_BYTES,
  parseKnowledgeCitations,
  validateLocalKnowledgeFile
} from "./knowledgeCitations.ts";

test("parseKnowledgeCitations extracts structured citations for direct and use_tool knowledge_search calls", () => {
  const payload = JSON.stringify({
    content_trust: "untrusted_data",
    mode: "hybrid",
    truncated: false,
    estimated_tokens: 84,
    hits: [
      {
        id: "11111111-1111-1111-1111-111111111111",
        source_label: "Ops Runbook",
        locator: "ops/restore.md#extracted-text",
        section: "Disaster Recovery > Database Restore",
        line_start: 14,
        line_end: 29,
        content: "Run pg_restore with --clean and verify replication lag before promoting the replica."
      },
      {
        id: "22222222-2222-2222-2222-222222222222",
        source_label: "Release Checklist",
        locator: "checklist.md",
        section: "",
        line_start: 7,
        line_end: 7,
        content: "Tag the release commit."
      }
    ]
  });

  const direct = parseKnowledgeCitations("knowledge_search", { query: "restore" }, payload);
  assert.ok(direct);
  assert.equal(direct.mode, "hybrid");
  assert.equal(direct.hits.length, 2);
  assert.equal(direct.hits[0]?.sourceLabel, "Ops Runbook");
  assert.equal(direct.hits[0]?.displayLocator, "ops/restore.md");
  assert.equal(direct.hits[0]?.section, "Disaster Recovery > Database Restore");
  assert.equal(direct.hits[0]?.lineRange, "L14–L29");
  assert.match(direct.hits[0]?.snippet ?? "", /pg_restore/);
  assert.equal(direct.hits[1]?.lineRange, "L7");

  const wrapped = parseKnowledgeCitations(
    "use_tool",
    { name: "knowledge_search", input: { query: "restore" } },
    payload
  );
  assert.deepEqual(wrapped, direct);

  assert.equal(parseKnowledgeCitations("web_search", { query: "x" }, payload), null);
  assert.equal(parseKnowledgeCitations("knowledge_search", {}, "not json"), null);
});

test("defaultKnowledgeSourceLabel derives labels for files, folders, URLs, chats and emails", () => {
  assert.equal(
    defaultKnowledgeSourceLabel("file", { path: "C:\\Users\\alice\\docs\\architecture.md" }),
    "architecture.md"
  );
  assert.equal(
    defaultKnowledgeSourceLabel("folder", { path: "/home/alice/projects/handbook/" }),
    "handbook"
  );
  assert.equal(
    defaultKnowledgeSourceLabel("saved_web", { url: "https://docs.example.com/guides/install/" }),
    "docs.example.com/guides/install"
  );
  assert.equal(
    defaultKnowledgeSourceLabel("conversation", { chatTitle: "Q4 planning" }),
    "Q4 planning"
  );
  assert.equal(
    defaultKnowledgeSourceLabel("email", { emailAccount: "ops@example.com" }),
    "Email snapshot · ops@example.com"
  );
  assert.equal(
    defaultKnowledgeSourceLabel("shared_brain", { defaultBrainName: "Shared brain" }),
    "Shared brain"
  );
  assert.equal(defaultKnowledgeSourceLabel("note", {}), "");
  assert.equal(
    defaultKnowledgeSourceLabel("file", { label: "Custom Title", path: "/tmp/a.md" }),
    "Custom Title"
  );
});

test("validateLocalKnowledgeFile enforces the 4 MiB limit and supported extensions", () => {
  assert.deepEqual(validateLocalKnowledgeFile("notes.md", 1024), {
    ok: true,
    html: false
  });
  assert.deepEqual(validateLocalKnowledgeFile("page.html", MAX_KNOWLEDGE_SOURCE_BYTES), {
    ok: true,
    html: true
  });
  assert.deepEqual(
    validateLocalKnowledgeFile("huge.md", MAX_KNOWLEDGE_SOURCE_BYTES + 1),
    { ok: false, reason: "too_large" }
  );
  assert.deepEqual(validateLocalKnowledgeFile("archive.exe", 512), {
    ok: false,
    reason: "unsupported"
  });
});
