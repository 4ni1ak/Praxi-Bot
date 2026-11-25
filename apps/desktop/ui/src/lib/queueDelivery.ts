import { putBack, removeQueued, type MessageQueue, type QueuedMessage } from "./messageQueue.ts";

/**
 * Hands queued messages to the daemon exactly once.
 *
 * Several effects of a chat (run end, connection back, "send now") can fire in the same commit on the
 * same stale snapshot of the queue. A message therefore leaves the queue atomically, inside the
 * store's own update, at the moment its delivery starts, and stays in an in-flight set until the
 * delivery is over. A second attempt for the same id finds it neither in the queue nor free, and does
 * nothing. A message that is not ready to go (no connection, another send running) is never touched:
 * it simply stays queued. Only the delivery itself can put its message back, and only once.
 */
export interface QueueDelivery {
  /**
   * Starts delivering `message` when `ready` and it is still queued and not in flight; returns whether
   * it started. `run` does the send; `hold(patch)` puts the message back (front of the queue), once.
   * A rejection of `run` also puts it back, as an uncertain send that is never replayed on its own.
   */
  deliver(
    message: QueuedMessage,
    ready: boolean,
    run: (message: QueuedMessage, hold: (patch?: Partial<QueuedMessage>) => void) => Promise<void>
  ): boolean;
  inFlight(id: string): boolean;
}

export function createQueueDelivery(update: (f: (q: MessageQueue) => MessageQueue) => void): QueueDelivery {
  const flying = new Set<string>();
  return {
    inFlight: (id) => flying.has(id),
    deliver(message, ready, run) {
      if (!ready || flying.has(message.id)) return false;
      let taken: QueuedMessage | null = null;
      update((q) => {
        taken = q.find((m) => m.id === message.id) ?? null;
        return taken ? removeQueued(q, message.id) : q;
      });
      const live = taken as QueuedMessage | null;
      if (!live) return false;
      flying.add(live.id);
      let held = false;
      const hold = (patch?: Partial<QueuedMessage>) => {
        if (held) return;
        held = true;
        update((q) => putBack(q, { ...live, ...patch }));
      };
      void run(live, hold)
        .catch(() => hold({ offline: false, stopEpoch: -1 }))
        .finally(() => flying.delete(live.id));
      return true;
    }
  };
}
