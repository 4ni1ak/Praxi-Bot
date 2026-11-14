import assert from "node:assert/strict";
import { test } from "node:test";
import { advanceKnowledgeEmbeddings } from "./knowledgeEmbedding.ts";
import type { KnowledgeEmbeddingProgress } from "../api/types";

const progress: KnowledgeEmbeddingProgress = { status: "progress", embedded_chunks: 1, cache_hits: 0, remaining_chunks: 20, next_attempt_ms: null, failure_code: null };

test("one explicit embedding retry never resets subsequent failures automatically", async () => {
  const resets: boolean[] = [];
  const statuses: string[] = [];
  await advanceKnowledgeEmbeddings(async (reset) => {
    resets.push(reset);
    return { ...progress, status: resets.length < 3 ? "progress" : "failed" };
  }, (value) => statuses.push(value.status));
  assert.deepEqual(resets, [true, false, false]);
  assert.deepEqual(statuses, ["progress", "progress", "failed"]);
});

test("embedding continuation is bounded and stops at a shared quota cooldown", async () => {
  const resets: boolean[] = [];
  await advanceKnowledgeEmbeddings(async (reset) => { resets.push(reset); return progress; }, () => {});
  assert.equal(resets.length, 16);
  assert.equal(resets.filter(Boolean).length, 1);
  resets.length = 0;
  await advanceKnowledgeEmbeddings(async (reset) => { resets.push(reset); return { ...progress, status: "deferred", next_attempt_ms: 123 }; }, () => {});
  assert.deepEqual(resets, [true]);
});
