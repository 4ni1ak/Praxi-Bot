/**
 * Messages typed while the bot is working (design v7): there is no "queue / steer" choice. A message
 * goes straight into the chat as a dashed "Sırada." bubble; "Şimdi gönder" delivers it now (the
 * daemon's steer path), otherwise it is delivered when the run ends (the daemon's queue path).
 * Pure, so the whole life of a queued message is tested without a browser.
 */

export interface QueuedMessage {
  id: string;
  text: string;
  createdMs: number;
  /** Retained only in memory until this chat's connection returns. */
  offline?: boolean;
  /** Stop decisions made after enqueueing hold automatic delivery across chat navigation. */
  stopEpoch?: number;
}

export type MessageQueue = readonly QueuedMessage[];

export const EMPTY_QUEUE: MessageQueue = [];
export const MAX_QUEUED_MESSAGES = 20;
export const MAX_QUEUED_CHARS = 32_000;

export function queueHasRoom(queue: MessageQueue, text: string): boolean {
  return queue.length < MAX_QUEUED_MESSAGES &&
    queue.reduce((n, m) => n + m.text.length, 0) + text.trim().length <= MAX_QUEUED_CHARS;
}

/**
 * Whether a message typed now waits in the local "Sirada" queue (true) or goes to the daemon at once.
 * While the run waits for a tool approval it must go to the daemon: that is what skips the card and
 * starts the next turn, so a local queue would hold it behind a run that cannot move. `approvalWaits`
 * is true when a card is open here OR the daemon says the run waits and no card reached this app
 * (a paired host's events can lag), so the decision never depends on the card having arrived.
 */
export function queuesLocally(a: { runActive: boolean; approvalWaits: boolean; slashCommand: boolean; hasFiles: boolean }): boolean {
  return a.runActive && !a.approvalWaits && !a.slashCommand && !a.hasFiles;
}

/** A message typed during a run joins the end of the queue (blank text never does). */
export function enqueue(queue: MessageQueue, text: string, id: string, now: number, offline = false, stopEpoch?: number): MessageQueue {
  const body = text.trim();
  return body && queueHasRoom(queue, body) ? [...queue, { id, text: body, createdMs: now, ...(offline ? { offline } : {}), ...(stopEpoch === undefined ? {} : { stopEpoch }) }] : queue;
}

export function canAutoDeliver(message: QueuedMessage, stopEpoch: number): boolean {
  return message.stopEpoch === undefined || message.stopEpoch === stopEpoch;
}

/** The small x on a queued bubble. */
export function removeQueued(queue: MessageQueue, id: string): MessageQueue {
  return queue.filter((m) => m.id !== id);
}

/** "Şimdi gönder": the message leaves the queue and is delivered now. */
export function takeForNow(queue: MessageQueue, id: string): { message: QueuedMessage | null; rest: MessageQueue } {
  const message = queue.find((m) => m.id === id) ?? null;
  return { message, rest: message ? removeQueued(queue, id) : queue };
}

/** A send that failed goes back to where it was (the front: it was the one being delivered). */
export function putBack(queue: MessageQueue, message: QueuedMessage): MessageQueue {
  return queue.some((m) => m.id === message.id) ? queue : [message, ...queue];
}

/**
 * When the run ends the oldest queued message is delivered; the rest keep waiting for the run it
 * starts, so they arrive in the order they were typed and each answer sees the one before it.
 * `canDeliver` holds back messages that may not go out on their own (they stay queued, in place).
 */
export function takeForRunEnd(
  queue: MessageQueue,
  wasActive: boolean,
  isActive: boolean,
  canDeliver: (message: QueuedMessage) => boolean = () => true
): { message: QueuedMessage | null; rest: MessageQueue } {
  const message = !wasActive || isActive ? undefined : queue.find(canDeliver);
  if (!message) {
    return { message: null, rest: queue };
  }
  return { message, rest: removeQueued(queue, message.id) };
}
