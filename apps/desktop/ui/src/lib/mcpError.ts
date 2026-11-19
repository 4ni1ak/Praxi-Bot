// Turns the class of an MCP start failure into a localized "what to do" hint.
// The daemon sends `error_kind` next to the raw `error` text (see McpServerDto).

export type McpErrorKind =
  | "runtime_missing"
  | "spawn"
  | "closed"
  | "timeout"
  | "config"
  | "needs_folder"
  | "other";

export type RuntimeFamily = "node" | "uv" | "pipx" | "docker" | "other";

/** Which installer a launcher command belongs to (`npx` -> Node.js, `uvx` -> uv ...). */
export function runtimeFamily(command: string | null | undefined): RuntimeFamily {
  const base = (command ?? "")
    .split(/[\\/]/)
    .pop()!
    .replace(/\.(exe|cmd|bat)$/i, "")
    .toLowerCase();
  if (base === "npx" || base === "npm" || base === "node") return "node";
  if (base === "uvx" || base === "uv") return "uv";
  if (base === "pipx") return "pipx";
  if (base === "docker") return "docker";
  return "other";
}

/** i18n key of the hint for a failed server, or null when there is no specific advice. */
export function mcpErrorHintKey(
  kind: McpErrorKind | null | undefined,
  command: string | null | undefined
): string | null {
  switch (kind) {
    case "runtime_missing":
      return `apps.error.hint.runtime_${runtimeFamily(command)}`;
    case "closed":
    case "spawn":
    case "timeout":
    case "config":
    case "needs_folder":
      return `apps.error.hint.${kind}`;
    default:
      return null;
  }
}
