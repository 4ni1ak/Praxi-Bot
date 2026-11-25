/**
 * Pure formatting and validation helpers for persistent bot workspace paths, sizes, and limits.
 */

export const STANDARD_WORKSPACE_SUBDIRS = new Set([
  "leads",
  "drafts",
  "downloads",
  "notes",
  "logs"
]);

const GIB = 1024 * 1024 * 1024;
const MIB = 1024 * 1024;
const KIB = 1024;

/** Human-readable binary byte size (e.g. `420 B`, `14.2 KB`, `3.4 GB`). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }
  if (bytes >= GIB) {
    return `${(bytes / GIB).toFixed(bytes >= 10 * GIB ? 1 : 2)} GB`;
  }
  if (bytes >= MIB) {
    return `${(bytes / MIB).toFixed(bytes >= 10 * MIB ? 1 : 2)} MB`;
  }
  if (bytes >= KIB) {
    return `${(bytes / KIB).toFixed(1)} KB`;
  }
  return `${Math.round(bytes)} B`;
}

/** Convert optional byte limit into a compact GB string for form inputs (`""` when off). */
export function bytesToGbInput(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes <= 0) {
    return "";
  }
  const gb = bytes / GIB;
  return Number.isInteger(gb) ? String(gb) : gb.toFixed(2).replace(/\.?0+$/, "");
}

/**
 * Parse a decimal GB input field into whole bytes (`null` when empty -> off; `undefined` on invalid).
 */
export function gbInputToBytes(raw: string): number | null | undefined {
  const trimmed = raw.trim();
  if (!trimmed) {
    return null;
  }
  const value = Number(trimmed);
  if (!Number.isFinite(value) || value <= 0) {
    return undefined;
  }
  return Math.round(value * GIB);
}

/** Build clickable breadcrumb segments from a relative workspace path (`a/b/c`). */
export function workspaceBreadcrumbs(relPath: string): { label: string; path: string }[] {
  const clean = relPath
    .split("/")
    .map((s) => s.trim())
    .filter((s) => s.length > 0 && s !== "." && s !== "..");
  const out: { label: string; path: string }[] = [];
  let acc = "";
  for (const part of clean) {
    acc = acc ? `${acc}/${part}` : part;
    out.push({ label: part, path: acc });
  }
  return out;
}

export function isStandardWorkspaceSubdir(relPath: string): boolean {
  return STANDARD_WORKSPACE_SUBDIRS.has(relPath);
}

export function usagePercent(usedBytes: number, limitBytes: number | null | undefined): number {
  if (!limitBytes || limitBytes <= 0 || !Number.isFinite(usedBytes)) {
    return 0;
  }
  return Math.min(100, Math.round((usedBytes / limitBytes) * 100));
}
