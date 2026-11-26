import { useEffect, useRef, useState } from "react";
import { firstUnseenMessageId, isAttended, lastReadMs } from "../lib/unread";
import { markChatRead, useReadState } from "../store/readStateStore";

function useAttended(): boolean {
  const [attended, setAttended] = useState(() => isAttended(document));
  useEffect(() => {
    const update = () => setAttended(isAttended(document));
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    document.addEventListener("visibilitychange", update);
    return () => {
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);
  return attended;
}

/**
 * Marks the open chat read while the user can see the window, and says where the "New messages"
 * divider applies: bot output newer than the read position the chat had when it was opened.
 */
export function useChatRead(
  host: string,
  chatId: string,
  activityMs: number,
  messages: readonly { id: string; role: string; created_at_ms: number }[],
  external = false
): { seenUntilMs: number | null; dismiss: () => void } {
  const state = useReadState(host);
  const attended = useAttended();
  const anchor = useRef<{ chatId: string; ms: number } | null>(null);
  if (anchor.current?.chatId !== chatId) anchor.current = { chatId, ms: lastReadMs(state, chatId) };
  const [dismissed, setDismissed] = useState<string | null>(null);
  const newest = messages.reduce((ms, m) => Math.max(ms, m.created_at_ms), activityMs);
  // Output that arrives while the owner is looking at this very chat is read as it arrives: the
  // divider only marks what was already waiting when the chat was opened (or came in while away).
  const watched = useRef<{ chatId: string; newest: number | null } | null>(null);
  if (watched.current?.chatId !== chatId) watched.current = { chatId, newest: null };
  if (messages.length > 0) {
    // The first loaded history is the baseline (the divider may sit in it); only later growth counts.
    if (attended && !external && watched.current.newest !== null && newest > watched.current.newest) {
      anchor.current = { chatId, ms: Math.max(anchor.current.ms, newest) };
    }
    if (attended || watched.current.newest === null) watched.current = { chatId, newest };
  }
  useEffect(() => {
    if (!external && attended && newest > 0) markChatRead(host, chatId, newest);
  }, [external, attended, host, chatId, newest]);
  const id = firstUnseenMessageId(messages, anchor.current.ms);
  const key = `${chatId}:${id}`;
  return { seenUntilMs: id && dismissed !== key ? anchor.current.ms : null, dismiss: () => setDismissed(key) };
}
