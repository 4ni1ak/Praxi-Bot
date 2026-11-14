import type { RestorePreview } from "../api/types.ts";

export interface DiffRow {
  kind: "hunk" | "add" | "del" | "context";
  oldLine: number | null;
  newLine: number | null;
  /** Raw text line (never interpreted as HTML/markdown). */
  text: string;
}

export interface RestoreGateState {
  needsOverwriteConfirm: boolean;
  canConfirm: boolean;
  requiresForce: boolean;
}

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

const EXT_TO_LANG: Record<string, string> = {
  rs: "rust",
  ts: "ts",
  tsx: "tsx",
  js: "js",
  jsx: "jsx",
  mjs: "js",
  cjs: "js",
  py: "python",
  json: "json",
  css: "css",
  scss: "css",
  html: "html",
  htm: "html",
  sh: "sh",
  bash: "bash",
  zsh: "bash",
  sql: "sql",
  toml: "toml",
  yaml: "yaml",
  yml: "yaml",
  md: "md"
};

/** Infer a `lib/highlight` language name from a filesystem path. */
export function languageForPath(filePath: string): string {
  const clean = filePath.split(/[?#]/)[0] ?? filePath;
  const slash = Math.max(clean.lastIndexOf("/"), clean.lastIndexOf("\\"));
  const base = slash >= 0 ? clean.slice(slash + 1) : clean;
  const dot = base.lastIndexOf(".");
  if (dot <= 0 || dot === base.length - 1) {
    return "";
  }
  const ext = base.slice(dot + 1).toLowerCase();
  return EXT_TO_LANG[ext] ?? ext;
}

/** Strip Windows `\\?\` verbatim prefixes for human-readable path display. */
export function displayPath(filePath: string): string {
  if (filePath.startsWith("\\\\?\\UNC\\")) {
    return `\\\\${filePath.slice(8)}`;
  }
  if (filePath.startsWith("\\\\?\\")) {
    return filePath.slice(4);
  }
  return filePath;
}

/**
 * Parse a unified diff string into structured line rows for pure text/span
 * rendering. Never parses or emits HTML.
 */
export function parseUnifiedDiff(unified: string): DiffRow[] {
  if (!unified) {
    return [];
  }
  const lines = unified.split(/\r?\n/);
  const rows: DiffRow[] = [];
  let oldCursor = 0;
  let newCursor = 0;
  let inHunk = false;

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i] ?? "";
    if (!inHunk && (line.startsWith("--- ") || line.startsWith("+++ "))) {
      continue;
    }
    const hunk = HUNK_RE.exec(line);
    if (hunk) {
      inHunk = true;
      oldCursor = Number.parseInt(hunk[1] ?? "1", 10);
      newCursor = Number.parseInt(hunk[2] ?? "1", 10);
      rows.push({
        kind: "hunk",
        oldLine: null,
        newLine: null,
        text: line
      });
      continue;
    }
    if (!inHunk) {
      continue;
    }
    if (i === lines.length - 1 && line === "") {
      continue;
    }
    if (line.startsWith("+")) {
      rows.push({
        kind: "add",
        oldLine: null,
        newLine: newCursor,
        text: line.slice(1)
      });
      newCursor += 1;
    } else if (line.startsWith("-")) {
      rows.push({
        kind: "del",
        oldLine: oldCursor,
        newLine: null,
        text: line.slice(1)
      });
      oldCursor += 1;
    } else if (line.startsWith(" ")) {
      rows.push({
        kind: "context",
        oldLine: oldCursor,
        newLine: newCursor,
        text: line.slice(1)
      });
      oldCursor += 1;
      newCursor += 1;
    } else if (line.startsWith("\\")) {
      rows.push({
        kind: "hunk",
        oldLine: null,
        newLine: null,
        text: line
      });
    }
  }
  return rows;
}

/**
 * Compute whether the restore confirmation dialog allows submitting, ensuring
 * newer user edits cannot be overwritten unless explicitly acknowledged.
 */
export function evaluateRestoreGate(
  preview: RestorePreview | null,
  overwriteChecked: boolean,
  busy: boolean,
  conflictRequiresConfirm = false
): RestoreGateState {
  if (!preview || busy) {
    return {
      needsOverwriteConfirm: false,
      canConfirm: false,
      requiresForce: false
    };
  }
  const needsOverwriteConfirm = conflictRequiresConfirm || preview.modified_since.length > 0;
  return {
    needsOverwriteConfirm,
    canConfirm: preview.entries.some((entry) => entry.accessible && entry.action !== "keep") && (!needsOverwriteConfirm || overwriteChecked),
    requiresForce: needsOverwriteConfirm && overwriteChecked
  };
}
