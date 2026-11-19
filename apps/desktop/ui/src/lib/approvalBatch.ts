/**
 * Pending approvals of one run, grouped for a single card.
 *
 * Only plain approvals are batched: a card that carries a payload to review (egress), a secret
 * fill, a schedule, a brain write or a control grant keeps its own card, because the user has to
 * read it. Identical calls (same tool, same input) are shown once and answered together.
 */
import type { ToolItem } from "../state/timeline.ts";

export interface BatchRow {
  /** Stable key: tool name plus canonical input. */
  key: string;
  /** Every pending call this row stands for (identical calls collapse into one row). */
  items: ToolItem[];
}

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** Canonical JSON: object keys sorted, so equal inputs compare equal. */
function canonical(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  if (isObject(v)) {
    return `{${Object.keys(v)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonical(v[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v) ?? "null";
}

/** A pending approval the user can answer without reading a special body. */
export function isBatchable(item: ToolItem): boolean {
  if (item.status !== "awaiting_approval") return false;
  const d = item.details;
  if (d === undefined || d === null) return true;
  // The daemon adds `why`, `once_only`, `permission`, `memory`; a `kind` means a special card.
  return isObject(d) && d.kind === undefined && d.control_grant === undefined;
}

/** Group the batchable pending items; identical calls share a row. Order of first appearance. */
export function groupPending(items: readonly ToolItem[]): BatchRow[] {
  const rows = new Map<string, BatchRow>();
  for (const item of items) {
    if (!isBatchable(item)) continue;
    const key = `${item.name}\u0000${canonical(item.input)}`;
    const row = rows.get(key);
    if (row) row.items.push(item);
    else rows.set(key, { key, items: [item] });
  }
  return [...rows.values()];
}

/** The batch card replaces the individual cards once at least two rows (or a repeat) are pending. */
export function shouldBatch(rows: readonly BatchRow[]): boolean {
  return rows.length >= 2 || rows.some((r) => r.items.length >= 2);
}

/** Call ids the batch card answers for a set of selected rows. */
export function callIdsOf(rows: readonly BatchRow[], selected: ReadonlySet<string>): string[] {
  return rows.filter((r) => selected.has(r.key)).flatMap((r) => r.items.map((i) => i.callId));
}
