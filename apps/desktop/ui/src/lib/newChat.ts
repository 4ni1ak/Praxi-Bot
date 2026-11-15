import type { ApiClient } from "../api/client.ts";

export type NewChatResult = { ok: true; chatId: string } | { ok: false; message: string };

/** Create a chat; a failure becomes a message instead of an exception (PR #95, finding 11). */
export async function createChatFor(
  client: Pick<ApiClient, "createChat">,
  botId: string,
  refreshChats: () => Promise<unknown>
): Promise<NewChatResult> {
  try {
    const chat = await client.createChat(botId);
    await refreshChats().catch(() => {});
    return { ok: true, chatId: chat.id };
  } catch (e) {
    return { ok: false, message: e instanceof Error ? e.message : String(e) };
  }
}

/**
 * Where a failed new chat is shown: the empty state renders it inline; every other entry point
 * (right panel, bot profile, chat view) has no such line, so it gets a toast.
 */
export function newChatFailureNeedsToast(routeKind: string): boolean {
  return routeKind !== "bot";
}
