import type { Chat, UpdateChatInput } from "../api/types.ts";
import { type BotLanding, botLanding, type LandingChat } from "./botLanding.ts";

/** Maximum title length accepted by `PATCH /v1/chats/{id}` (trimmed Unicode scalar values). */
export const CHAT_TITLE_MAX_LEN = 120;

const FINISHED_RUN_STATES = new Set(["succeeded", "failed", "cancelled"]);

export type TitleValidation =
  | { ok: true; title: string }
  | { ok: false; reason: "empty" | "too_long" };

/** Validate and trim a chat title before sending `PATCH /v1/chats/{id}`. */
export function validateChatTitle(raw: string): TitleValidation {
  const title = raw.trim();
  if (!title) return { ok: false, reason: "empty" };
  if ([...title].length > CHAT_TITLE_MAX_LEN) return { ok: false, reason: "too_long" };
  return { ok: true, title };
}

export interface OrderableChat {
  pinned?: boolean;
  archived?: boolean;
  created_at_ms?: number;
  createdAtMs?: number;
}

const createdMs = (c: OrderableChat) => c.createdAtMs ?? c.created_at_ms ?? 0;

/** Sort chats with pinned items floating to the top, then newest first. */
export function orderChats<C extends OrderableChat>(chats: readonly C[]): C[] {
  return [...chats].sort(
    (a, b) => (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0) || createdMs(b) - createdMs(a)
  );
}

/** Split chats into visible (active) and archived groups, each pinned-first then newest. */
export function partitionChats<C extends OrderableChat>(chats: readonly C[]): { active: C[]; archived: C[] } {
  const active: C[] = [];
  const archived: C[] = [];
  for (const c of chats) (c.archived ? archived : active).push(c);
  return { active: orderChats(active), archived: orderChats(archived) };
}

/** Mirrors `DELETE /v1/chats/{id}` active-run check so the confirm dialog warns upfront. */
export function chatHasActiveRun(
  chatId: string,
  runs: ReadonlyArray<{ chat_id?: string | null; state: string }>
): boolean {
  return runs.some((r) => r.chat_id === chatId && !FINISHED_RUN_STATES.has(r.state));
}

/** Optimistically apply a chat patch and return the previous chat snapshot for rollback. */
export function applyChatPatch(
  chats: readonly Chat[],
  chatId: string,
  patch: UpdateChatInput
): { next: Chat[]; previous: Chat | null } {
  let previous: Chat | null = null;
  const next = chats.map((c) => {
    if (c.id !== chatId) return c;
    previous = c;
    return {
      ...c,
      ...(patch.title != null ? { title: patch.title } : {}),
      ...(patch.pinned != null ? { pinned: patch.pinned } : {}),
      ...(patch.archived != null ? { archived: patch.archived } : {})
    };
  });
  return { next, previous };
}

export function rollbackChatPatch(chats: readonly Chat[], previous: Chat | null): Chat[] {
  return previous ? chats.map((c) => (c.id === previous.id ? previous : c)) : [...chats];
}

export function applyChatDelete(
  chats: readonly Chat[],
  chatId: string
): { next: Chat[]; removed: Chat | null; index: number } {
  const index = chats.findIndex((c) => c.id === chatId);
  if (index === -1) return { next: [...chats], removed: null, index: -1 };
  return { next: [...chats.slice(0, index), ...chats.slice(index + 1)], removed: chats[index] ?? null, index };
}

export function rollbackChatDelete(chats: readonly Chat[], removed: Chat | null, index: number): Chat[] {
  if (!removed || chats.some((c) => c.id === removed.id)) return [...chats];
  const at = Math.max(0, Math.min(index, chats.length));
  return [...chats.slice(0, at), removed, ...chats.slice(at)];
}

/** Next route after deleting a chat: switches away only if that chat was open. */
export function nextRouteAfterChatDelete(
  deletedChatId: string,
  botId: string,
  remainingChats: readonly LandingChat[],
  currentRoute: { kind: string; chatId?: string }
): BotLanding | null {
  return currentRoute.kind === "chat" && currentRoute.chatId === deletedChatId
    ? botLanding(botId, remainingChats)
    : null;
}
