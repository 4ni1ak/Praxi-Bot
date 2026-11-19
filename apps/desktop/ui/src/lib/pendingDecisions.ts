import {
  topLevelRuns,
  type EventsState,
  type NeedsYouEntry
} from "../state/timeline.ts";

/** How the pending cards split for one chat. */
export interface ChatPending {
  /** This chat's cards that the timeline draws at their own place (its top-level runs). */
  inline: NeedsYouEntry[];
  /**
   * Cards pinned above the composer so none can be missing: this chat's cards the timeline cannot
   * draw (a subagent's, a run the events never placed) and the bot's cards from its other chats.
   */
  pinned: NeedsYouEntry[];
  /** Everything that belongs to this chat (inline + its pinned ones): what the status line names. */
  own: NeedsYouEntry[];
}

/**
 * One entry, one card: an entry is either drawn inline or pinned, never both, and never neither
 * (a card that waits for the user and shows nowhere is the bug this guards).
 */
export function pendingForChat(
  needs: readonly NeedsYouEntry[],
  events: EventsState,
  chat: { id: string; botId: string | undefined }
): ChatPending {
  const topLevel = new Set(topLevelRuns(events, chat.id).map((r) => r.id));
  const inline: NeedsYouEntry[] = [];
  const pinned: NeedsYouEntry[] = [];
  const own: NeedsYouEntry[] = [];
  for (const entry of needs) {
    if (entry.chatId === chat.id) {
      own.push(entry);
      (topLevel.has(entry.runId) ? inline : pinned).push(entry);
    } else if (
      chat.botId !== undefined &&
      entry.botId === chat.botId &&
      entry.chatId !== chat.id
    ) {
      pinned.push(entry);
    }
  }
  return { inline, pinned, own };
}
