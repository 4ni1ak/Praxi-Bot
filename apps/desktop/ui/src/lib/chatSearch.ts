import { chatTitleLabel } from "./chatTitle.ts";

/** Show the chat search box once a bot has more chats than fit at a glance. */
export const CHAT_SEARCH_MIN_CHATS = 6;

/** Chats whose displayed title contains `query` (case-insensitive); a blank query keeps all. */
export function filterChats<T extends { title: string }>(
  chats: readonly T[],
  query: string,
  untitledLabel: string
): T[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) {
    return [...chats];
  }
  return chats.filter((c) => chatTitleLabel(c.title, untitledLabel).toLocaleLowerCase().includes(needle));
}
