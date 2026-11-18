/**
 * Structured citation parsing for `knowledge_search` tool results and helper
 * utilities for knowledge source labels and local file size/format validation.
 */

import type { KnowledgeSourceKind } from "../api/types.ts";
import { resolveToolCall } from "./toolLabels.ts";

/** Maximum input bytes per knowledge file or saved web snapshot (4 MiB). */
export const MAX_KNOWLEDGE_SOURCE_BYTES = 4 * 1024 * 1024;
/** Maximum extracted UTF-8 text bytes indexed per knowledge document (1 MiB). */
export const MAX_KNOWLEDGE_TEXT_BYTES = 1024 * 1024;

const SUPPORTED_TEXT_EXTENSIONS = new Set([
  "md",
  "markdown",
  "txt",
  "html",
  "htm",
  "rs",
  "php",
  "js",
  "jsx",
  "ts",
  "tsx",
  "py",
  "go",
  "java",
  "c",
  "h",
  "cpp",
  "hpp",
  "cs",
  "sql",
  "css",
  "scss",
  "json",
  "toml",
  "yaml",
  "yml",
  "sh",
  "csv"
]);

export interface KnowledgeCitationHit {
  id: string;
  sourceLabel: string;
  locator: string;
  displayLocator: string;
  section: string;
  lineStart: number;
  lineEnd: number;
  lineRange: string;
  snippet: string;
}

export interface KnowledgeCitationBlock {
  mode: "lexical" | "hybrid";
  truncated: boolean;
  estimatedTokens: number;
  hits: KnowledgeCitationHit[];
}

function clipText(raw: string, max = 180): string {
  const flat = raw.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

/**
 * Parse a `knowledge_search` (or `use_tool { name: "knowledge_search" }`) JSON
 * output preview into structured citations for the chat tool card.
 */
export function parseKnowledgeCitations(
  toolName: string,
  toolInput: unknown,
  outputPreview?: string
): KnowledgeCitationBlock | null {
  const resolved = resolveToolCall(toolName, toolInput);
  if (resolved.name !== "knowledge_search" || !outputPreview) {
    return null;
  }
  const trimmed = outputPreview.trim();
  if (!trimmed.startsWith("{")) {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return null;
  }
  const obj = parsed as Record<string, unknown>;
  if (!Array.isArray(obj.hits)) {
    return null;
  }
  const hits: KnowledgeCitationHit[] = [];
  for (let i = 0; i < obj.hits.length; i++) {
    const raw = obj.hits[i];
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
      continue;
    }
    const item = raw as Record<string, unknown>;
    const sourceLabel =
      typeof item.source_label === "string" && item.source_label.trim()
        ? item.source_label.trim()
        : "";
    const locator =
      typeof item.locator === "string" && item.locator.trim()
        ? item.locator.trim()
        : "";
    if (!sourceLabel && !locator) {
      continue;
    }
    const displayLocator = locator.replace(/#extracted-text$/, "");
    const section =
      typeof item.section === "string" ? item.section.trim() : "";
    const lineStart =
      typeof item.line_start === "number" && Number.isFinite(item.line_start)
        ? Math.max(1, Math.trunc(item.line_start))
        : 1;
    const lineEnd =
      typeof item.line_end === "number" && Number.isFinite(item.line_end)
        ? Math.max(lineStart, Math.trunc(item.line_end))
        : lineStart;
    const lineRange =
      lineEnd > lineStart ? `L${lineStart}–L${lineEnd}` : `L${lineStart}`;
    const content =
      typeof item.content === "string" ? clipText(item.content) : "";
    hits.push({
      id: typeof item.id === "string" && item.id ? item.id : `hit-${i}`,
      sourceLabel: sourceLabel || displayLocator,
      locator,
      displayLocator: displayLocator || sourceLabel,
      section,
      lineStart,
      lineEnd,
      lineRange,
      snippet: content
    });
  }
  const mode = obj.mode === "hybrid" ? "hybrid" : "lexical";
  const truncated = Boolean(obj.truncated);
  const estimatedTokens =
    typeof obj.estimated_tokens === "number" &&
    Number.isFinite(obj.estimated_tokens)
      ? Math.max(0, Math.trunc(obj.estimated_tokens))
      : 0;
  return { mode, truncated, estimatedTokens, hits };
}

/**
 * Derive a default knowledge source label when the user leaves the name field
 * blank, matching the daemon's `default_source_label` logic.
 */
export function defaultKnowledgeSourceLabel(
  kind: KnowledgeSourceKind,
  fields: {
    label?: string;
    path?: string;
    url?: string;
    chatTitle?: string;
    emailAccount?: string;
    defaultBrainName?: string;
  }
): string {
  const explicit = fields.label?.trim() ?? "";
  if (explicit) {
    return explicit.slice(0, 200);
  }
  let derived = "";
  switch (kind) {
    case "shared_brain":
      derived = fields.defaultBrainName?.trim() || "Shared brain";
      break;
    case "file":
    case "folder": {
      const parts = (fields.path ?? "")
        .split(/[\\/]/)
        .map((s) => s.trim())
        .filter(Boolean);
      derived = parts[parts.length - 1] ?? "";
      break;
    }
    case "saved_web": {
      const rawUrl = fields.url?.trim() ?? "";
      if (rawUrl) {
        try {
          const parsed = new URL(rawUrl);
          const cleanPath = parsed.pathname.replace(/\/+$/, "");
          derived =
            cleanPath && cleanPath !== "/"
              ? `${parsed.hostname}${cleanPath}`
              : parsed.hostname;
        } catch {
          derived = rawUrl;
        }
      }
      break;
    }
    case "conversation":
      derived = fields.chatTitle?.trim() ?? "";
      break;
    case "email": {
      const account = fields.emailAccount?.trim() ?? "";
      derived = account ? `Email snapshot · ${account}` : "";
      break;
    }
    case "note":
      derived = "";
      break;
  }
  return derived.slice(0, 200);
}

/**
 * Validate a local text/HTML/code file selected in the browser before reading
 * its contents into a knowledge source snapshot.
 */
export function validateLocalKnowledgeFile(
  fileName: string,
  sizeBytes: number
):
  | { ok: true; html: boolean }
  | { ok: false; reason: "too_large" | "unsupported" } {
  if (sizeBytes > MAX_KNOWLEDGE_SOURCE_BYTES) {
    return { ok: false, reason: "too_large" };
  }
  const dot = fileName.lastIndexOf(".");
  const ext = dot >= 0 ? fileName.slice(dot + 1).toLowerCase() : "";
  if (!SUPPORTED_TEXT_EXTENSIONS.has(ext)) {
    return { ok: false, reason: "unsupported" };
  }
  return { ok: true, html: ext === "html" || ext === "htm" };
}
