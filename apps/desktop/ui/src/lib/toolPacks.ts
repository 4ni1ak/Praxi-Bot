import type { HostPackInfo, HostSetupJob, PackPlanItem } from "../api/types";

/**
 * "Tools" for a server: pure helpers for the pack list, the size line, the progress rows and the
 * plain-language reasons. No screen code, so the flow is testable without rendering anything.
 */

/**
 * Rough download of the wizard's basic set (office + docs + media + dev), shown next to its switch
 * before any server is known. Mirrors the catalog's base packs; a daemon test keeps them in step.
 */
export const BASE_SET_MB = 1350;

/** Catalog ids in the order they are shown (the daemon owns the catalog; the app owns the words). */
export const PACK_ORDER = ["office", "docs", "ocr", "media", "dev"] as const;
export type PackId = (typeof PACK_ORDER)[number];

export function isPackId(id: string): id is PackId {
  return (PACK_ORDER as readonly string[]).includes(id);
}

/** Packs in display order; ids this app version has no words for are kept last, never dropped. */
export function sortedPacks<T extends { id: string }>(packs: readonly T[]): T[] {
  const rank = (id: string) => {
    const i = (PACK_ORDER as readonly string[]).indexOf(id);
    return i === -1 ? PACK_ORDER.length : i;
  };
  return [...packs].sort((a, b) => rank(a.id) - rank(b.id));
}

/** "about 700 MB" / "about 1.2 GB" as `{ value, unit }` so the language file owns the sentence. */
export function sizeParts(mb: number): { value: string; unit: "MB" | "GB" } {
  if (mb >= 1000) return { value: (Math.round(mb / 100) / 10).toFixed(1).replace(/\.0$/, ""), unit: "GB" };
  return { value: String(Math.max(10, Math.round(mb / 10) * 10)), unit: "MB" };
}

export function totalMb(packs: readonly HostPackInfo[], selected: readonly string[]): number {
  return packs.filter((p) => selected.includes(p.id)).reduce((sum, p) => sum + p.size_mb, 0);
}

/** Packs the one-switch basic set would still add (base packs that are not complete yet). */
export function missingBase(packs: readonly HostPackInfo[]): string[] {
  return packs.filter((p) => p.base && p.status !== "installed").map((p) => p.id);
}

export type PackStatus = "installed" | "partial" | "missing";

export function statusOf(p: Pick<HostPackInfo, "status">): PackStatus {
  return p.status === "installed" || p.status === "partial" ? p.status : "missing";
}

/** Reasons the app has a sentence for; anything else reads as a generic problem. */
const REASONS = [
  "needs_admin",
  "unsupported",
  "connection",
  "unavailable",
  "shared",
  "dependants",
  "blocked_shared",
  "blocked_dependants",
  "unknown_pack",
  "failed"
] as const;
export type Reason = (typeof REASONS)[number];

export function reasonOf(code: string | null | undefined): Reason {
  const c = (code ?? "").toLowerCase();
  return (REASONS as readonly string[]).includes(c) ? (c as Reason) : "failed";
}

/** The reason `shared` (from a plan) and `blocked_shared` (from a run) are the same sentence. */
export function reasonKey(code: string | null | undefined): Exclude<Reason, "blocked_shared" | "blocked_dependants"> {
  const r = reasonOf(code);
  if (r === "blocked_shared") return "shared";
  if (r === "blocked_dependants") return "dependants";
  return r;
}

export type RowState = "pending" | "running" | "done" | "failed";

/**
 * One row per planned pack. A pack is done or failed once its `pack.<id>` step exists; the first
 * one without a step is the one running, the rest wait. After a failure nothing else runs.
 */
export function rowStates(job: HostSetupJob | null, ids: readonly string[]): Record<string, RowState> {
  const out: Record<string, RowState> = {};
  let running = job === null || (job.state !== "failed" && job.state !== "cancelled");
  for (const id of ids) {
    const step = job?.steps.find((s) => s.id === `pack.${id}`);
    if (step) {
      out[id] = step.status === "pass" || step.status === "skip" ? "done" : "failed";
    } else if (running) {
      out[id] = "running";
      running = false;
    } else {
      out[id] = "pending";
    }
  }
  return out;
}

/** The reason of the first failed step of a pack job (`null` when nothing failed). */
export function jobFailure(job: HostSetupJob | null): Reason | null {
  const bad = job?.steps.find((s) => s.status === "fail" || s.status === "warn");
  return bad ? reasonOf(bad.detail) : null;
}

/** Plan items that can run. */
export function runnableItems(items: readonly PackPlanItem[]): PackPlanItem[] {
  return items.filter((i) => !i.noop && !i.blocked);
}
