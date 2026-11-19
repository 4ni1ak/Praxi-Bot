import type { McpTool } from "../api/types.ts";

/** One row of a server's tool list: a tool and whether the bots may call it. */
export interface McpToolRow {
  /** The server's own tool name (what `tool_exclude` matches). */
  tool: string;
  enabled: boolean;
  effect: string;
  description: string;
}

/** `mcp:<server>/<tool>` -> `<tool>`. */
export function rawToolName(qualified: string): string {
  const slash = qualified.indexOf("/");
  return slash >= 0 ? qualified.slice(slash + 1) : qualified;
}

const GLOB = /[*?[\]{}]/;

/**
 * Rows for the tool switches. The daemon lists only tools that pass the
 * server's filters, so excluded tools come back from `tool_exclude` itself
 * (literal names only; a glob pattern is left alone). Returns `null` when
 * the server has an allow-list: that is a deliberate restriction the switches
 * must not silently rewrite.
 */
export function toolRows(
  tools: McpTool[],
  exclude: string[],
  allow: string[]
): McpToolRow[] | null {
  if (allow.length > 0) {
    return null;
  }
  const rows: McpToolRow[] = tools.map((t) => ({
    tool: rawToolName(t.name),
    enabled: true,
    effect: t.effect,
    description: t.description
  }));
  const seen = new Set(rows.map((r) => r.tool));
  for (const name of exclude) {
    if (!GLOB.test(name) && !seen.has(name)) {
      rows.push({ tool: name, enabled: false, effect: "", description: "" });
      seen.add(name);
    }
  }
  return rows.sort((a, b) => a.tool.localeCompare(b.tool));
}

/** The new `tool_exclude` after switching one tool on or off. */
export function nextExclude(exclude: string[], tool: string, enabled: boolean): string[] {
  const rest = exclude.filter((e) => e !== tool);
  return enabled ? rest : [...rest, tool];
}
