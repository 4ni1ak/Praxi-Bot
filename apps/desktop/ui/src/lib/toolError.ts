/**
 * A tool failure as a sentence for the chat: the daemon's wording is for the model, so the
 * "observation paused" refusal gets a human line and the transport prefixes are dropped.
 */
export function humanToolError(raw: string, t: (key: string) => string): string {
  if (/observation paused/i.test(raw)) {
    return t("chat.tool.errors.observationPaused");
  }
  return raw.replace(/^Error:\s*/, "").replace(/^execution error:\s*/i, "");
}
