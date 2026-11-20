/** A local model server the `bot` CLI found on loopback (`bot providers detect-local`). */
export interface LocalServer {
  /** Provider id: `ollama`, `lmstudio` or `llamacpp`. */
  id: string;
  baseUrl: string;
  /** Installed model ids; empty when the server runs with nothing installed. */
  models: string[];
}

/** Small model suggested when a local server has nothing installed. */
export const SUGGESTED_LOCAL_MODEL = "llama3.2:3b";

const KNOWN = new Set(["ollama", "lmstudio", "llamacpp"]);

/** Parses the CLI's JSON array; anything malformed or unknown is dropped, never thrown. */
export function parseLocalServers(stdout: string): LocalServer[] {
  let raw: unknown;
  try {
    raw = JSON.parse(stdout);
  } catch {
    return [];
  }
  if (!Array.isArray(raw)) {
    return [];
  }
  const out: LocalServer[] = [];
  for (const item of raw) {
    const o = item as { id?: unknown; base_url?: unknown; models?: unknown };
    if (typeof o?.id === "string" && KNOWN.has(o.id) && typeof o.base_url === "string" && Array.isArray(o.models)) {
      out.push({ id: o.id, baseUrl: o.base_url, models: o.models.filter((m): m is string => typeof m === "string") });
    }
  }
  return out;
}
