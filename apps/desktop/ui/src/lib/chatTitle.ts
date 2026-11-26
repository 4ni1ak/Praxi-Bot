/** The title the daemon gives a chat before anything names it (stored in English). */
export const DEFAULT_CHAT_TITLE = "New chat";

/**
 * What to show for a chat title: a chat still carrying the daemon's default (or no title) shows the
 * UI language's own "new chat" text; a title that was set by a person or a bot is shown as it is.
 */
export function chatTitleLabel(title: string | null | undefined, newChatLabel: string): string {
  const trimmed = (title ?? "").trim();
  return trimmed === "" || trimmed === DEFAULT_CHAT_TITLE ? newChatLabel : trimmed;
}

/** Longest automatic title, in characters (the daemon accepts 120). */
export const AUTO_TITLE_MAX = 48;

/**
 * A title for a chat that still has the default one, taken from the owner's first message: its first
 * line without markdown marks, cut at a word.  `null` for messages that make no title (empty, a
 * `/command`, nothing but symbols).  Local and free: no model call.
 */
export function autoChatTitle(firstMessage: string | null | undefined): string | null {
  const line = (firstMessage ?? "").split(/\r?\n/).map((l) => l.trim()).find((l) => l !== "") ?? "";
  if (line.startsWith("/")) return null;
  const clean = line
    .replace(/https?:\/\/\S+/g, " ")
    .replace(/[`*_#>~[\]|]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!/[\p{L}\p{N}]/u.test(clean)) return null;
  const chars = [...clean];
  if (chars.length <= AUTO_TITLE_MAX) return clean.replace(/[.,;:!?]+$/u, "");
  const head = chars.slice(0, AUTO_TITLE_MAX).join("");
  const cut = head.lastIndexOf(" ");
  return `${(cut > AUTO_TITLE_MAX / 2 ? head.slice(0, cut) : head).replace(/[.,;:!?]+$/u, "")}…`;
}
