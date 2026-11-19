import type { RunState } from "../state/timeline";

/** The last reply text of a run that ended normally, or null (still running, stopped, failed, silent). */
export function finalReplyText(run: Pick<RunState, "finishedMs" | "error" | "stopped" | "items">): string | null {
  if (run.finishedMs === undefined || run.error || run.stopped) {
    return null;
  }
  for (let i = run.items.length - 1; i >= 0; i--) {
    const it = run.items[i];
    if (it?.kind === "text" && it.text.trim()) {
      return it.text;
    }
  }
  return null;
}

/**
 * Runs whose reply should be spoken now: finished since the last call and not seen before. Every
 * finished run is added to `seen`, so a reply is spoken once and old history is never read out.
 * With `prime` (first look at a chat) nothing is returned, only remembered.
 */
export function repliesToSpeak(
  runs: readonly Pick<RunState, "id" | "finishedMs" | "error" | "stopped" | "items">[],
  seen: Set<string>,
  prime: boolean
): { id: string; text: string }[] {
  const out: { id: string; text: string }[] = [];
  for (const r of runs) {
    if (r.finishedMs === undefined || seen.has(r.id)) {
      continue;
    }
    seen.add(r.id);
    const text = finalReplyText(r);
    if (text && !prime) {
      out.push({ id: r.id, text });
    }
  }
  return out;
}
