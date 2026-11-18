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
