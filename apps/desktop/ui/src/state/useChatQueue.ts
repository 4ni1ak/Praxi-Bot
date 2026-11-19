import { useCallback, useSyncExternalStore } from "react";
import { EMPTY_QUEUE, type MessageQueue } from "../lib/messageQueue";

/**
 * The queued messages of every chat, for the life of the app: leaving a chat and coming back keeps
 * them (they are delivered while the chat is open, when its run ends).
 */
const store = new Map<string, MessageQueue>();
const listeners = new Set<() => void>();

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function useChatQueue(chatId: string): [MessageQueue, (f: (q: MessageQueue) => MessageQueue) => void] {
  const queue = useSyncExternalStore(
    subscribe,
    () => store.get(chatId) ?? EMPTY_QUEUE,
    () => EMPTY_QUEUE
  );
  const update = useCallback(
    (f: (q: MessageQueue) => MessageQueue) => {
      const next = f(store.get(chatId) ?? EMPTY_QUEUE);
      if (next.length === 0) {
        store.delete(chatId);
      } else {
        store.set(chatId, next);
      }
      for (const l of listeners) {
        l();
      }
    },
    [chatId]
  );
  return [queue, update];
}
