import type { KnowledgeEmbeddingProgress } from "../api/types";

/** One explicit user action advances at most sixteen bounded batches. Only its
 * first batch resets exhausted retries; deferred work resumes on another click. */
export async function advanceKnowledgeEmbeddings(
  batch: (resetRetry: boolean) => Promise<KnowledgeEmbeddingProgress>,
  onProgress: (progress: KnowledgeEmbeddingProgress) => void
): Promise<void> {
  for (let index = 0; index < 16; index += 1) {
    const progress = await batch(index === 0);
    onProgress(progress);
    if (progress.status !== "progress") return;
  }
}
