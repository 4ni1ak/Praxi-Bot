const ATTACHMENT_MARKER_RE = /\n*\[\[praxi-attachments:[^\]]*\]\]\s*$/g;
const INTERNAL_TOOL_XML_RE = /<(?:tool_call|tool_response|function_calls)>[\s\S]*?<\/(?:tool_call|tool_response|function_calls)>/gi;
const TOOL_BLOCK_TYPES = new Set([
  "tool_use",
  "tool_result",
  "tool_call",
  "function_call",
  "function_call_output"
]);

function isInternalToolObject(value: unknown): boolean {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const rec = value as Record<string, unknown>;
  if (Array.isArray(rec.tool_calls)) {
    return true;
  }
  if (typeof rec.tool_call_id === "string") {
    return true;
  }
  if (typeof rec.call_id === "string" && typeof rec.tool_name === "string") {
    return true;
  }
  return typeof rec.type === "string" && TOOL_BLOCK_TYPES.has(rec.type);
}

function isInternalToolPayload(value: unknown): boolean {
  if (Array.isArray(value)) {
    return value.length > 0 && value.every(isInternalToolObject);
  }
  return isInternalToolObject(value);
}

/**
 * Returns the user-visible text of a chat message (plain text or Markdown),
 * stripping internal attachment markers and never exposing raw internal/tool JSON.
 */
export function visibleMessageText(raw: string): string {
  const cleaned = raw
    .replace(ATTACHMENT_MARKER_RE, "")
    .replace(INTERNAL_TOOL_XML_RE, "")
    .trim();
  if (!cleaned) {
    return "";
  }
  if (cleaned.startsWith("{") || cleaned.startsWith("[")) {
    try {
      const parsed: unknown = JSON.parse(cleaned);
      if (isInternalToolPayload(parsed)) {
        return "";
      }
    } catch {
      // Not valid JSON: keep the visible text as-is.
    }
  }
  return cleaned;
}

/** Puts visible message text on the clipboard; resolves false (never throws) when empty or refused. */
export async function copyText(
  text: string,
  clipboard: Pick<Clipboard, "writeText"> | undefined = typeof navigator === "undefined" ? undefined : navigator.clipboard
): Promise<boolean> {
  const visible = visibleMessageText(text);
  if (!clipboard || !visible) {
    return false;
  }
  try {
    await clipboard.writeText(visible);
    return true;
  } catch {
    return false;
  }
}
