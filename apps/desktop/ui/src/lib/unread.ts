/**
 * Unread markers.  The daemon keeps no read state, so the app remembers, per server and chat, the
 * time up to which the user has seen the chat (a convenience kept in this profile only).  A chat is
 * unread when a bot finished a run or asked for a decision after that time.
 */
export interface ReadState {
  /** Chats without a record count as read up to here (first sight of the server), so an old install is not all unread. */
  baseline: number;
  chats: Record<string, number>;
}

const MAX_CHATS = 500;

export function emptyReadState(now: number): ReadState {
  return { baseline: now, chats: {} };
}

export function parseReadState(raw: unknown, now: number): ReadState {
  const r = (raw ?? {}) as { baseline?: unknown; chats?: unknown };
  const baseline = typeof r.baseline === "number" && Number.isFinite(r.baseline) ? r.baseline : now;
  const chats: Record<string, number> = {};
  if (r.chats && typeof r.chats === "object") {
    for (const [id, ms] of Object.entries(r.chats as Record<string, unknown>)) {
      if (typeof ms === "number" && Number.isFinite(ms)) chats[id] = ms;
    }
  }
  return { baseline, chats };
}

export function lastReadMs(state: ReadState, chatId: string): number {
  return state.chats[chatId] ?? state.baseline;
}

/** Move a chat's read position forward (never back); returns the same object when nothing changes. */
export function withRead(state: ReadState, chatId: string, ms: number): ReadState {
  if (!(ms > lastReadMs(state, chatId))) return state;
  const chats = { ...state.chats, [chatId]: ms };
  const ids = Object.keys(chats);
  if (ids.length > MAX_CHATS) {
    ids.sort((a, b) => chats[a] - chats[b]).slice(0, ids.length - MAX_CHATS).forEach((id) => delete chats[id]);
  }
  return { ...state, chats };
}

interface RunFact {
  chat_id?: string | null;
  parent_run_id?: string | null;
  state: string;
  updated_at_ms: number;
}
interface NeedFact {
  chatId: string;
  sinceMs: number;
}

const LIVE = new Set(["pending", "running", "waiting_approval"]);

/** Latest time a bot produced something for the user in this chat: a run that ended, or a decision it waits for. */
export function chatActivityMs(runs: readonly RunFact[], needs: readonly NeedFact[], chatId: string): number {
  let ms = 0;
  for (const r of runs) {
    if (r.chat_id === chatId && !r.parent_run_id && !LIVE.has(r.state)) ms = Math.max(ms, r.updated_at_ms);
  }
  for (const n of needs) if (n.chatId === chatId) ms = Math.max(ms, n.sinceMs);
  return ms;
}

/** Bots with at least one chat that has activity the user has not seen. */
export function unreadBotIds(
  chats: readonly { id: string; bot_id: string }[],
  runs: readonly RunFact[],
  needs: readonly NeedFact[],
  state: ReadState
): Set<string> {
  const out = new Set<string>();
  for (const c of chats) {
    if (!out.has(c.bot_id) && chatActivityMs(runs, needs, c.id) > lastReadMs(state, c.id)) out.add(c.bot_id);
  }
  return out;
}

/** Id of the first bot message the user has not seen (the "New messages" divider goes above it), or null. */
export function firstUnseenMessageId(
  messages: readonly { id: string; role: string; created_at_ms: number }[],
  seenUntilMs: number
): string | null {
  return messages.find((m) => m.role === "assistant" && m.created_at_ms > seenUntilMs)?.id ?? null;
}

/** Reading only counts while the user can actually see the window. */
export function isAttended(doc: { visibilityState: string; hasFocus: () => boolean }): boolean {
  return doc.visibilityState === "visible" && doc.hasFocus();
}
