/**
 * Where selecting a bot leads (owner feedback 244, point 4): the bot "home page" is gone, so a
 * bot opens its latest chat, or - without any chat - an empty state with one action.  Everything
 * the home page offered lives in the right panel.
 */

export interface LandingChat {
  id: string;
  bot_id: string;
  created_at_ms: number;
  title?: string | null;
}

export type BotLanding =
  | { kind: "chat"; chatId: string; botId: string }
  /** No chat yet: the empty state ("new chat" is its only action). */
  | { kind: "bot"; botId: string };

/** The bot's chats, newest first. */
export function botChats<C extends LandingChat>(chats: readonly C[], botId: string): C[] {
  return chats
    .filter((c) => c.bot_id === botId)
    .sort((a, b) => b.created_at_ms - a.created_at_ms);
}

/** The route a bot opens on. */
export function botLanding(botId: string, chats: readonly LandingChat[]): BotLanding {
  const latest = botChats(chats, botId)[0];
  return latest ? { kind: "chat", chatId: latest.id, botId } : { kind: "bot", botId };
}

/**
 * Leaving a chat ("back"): there is no bot page to go back to, so the caller opens the right
 * panel, where the chat list lives.  Returns whether the panel has to be opened.
 */
export function backFromChatOpensPanel(panelOpen: boolean): boolean {
  return !panelOpen;
}

export type ChatActivity = "needs" | "running" | null;

/** Badge of one chat row in the panel's chat list. */
export function chatActivity(
  chatId: string,
  needs: ReadonlyArray<{ chatId: string }>,
  runs: ReadonlyArray<{ chat_id?: string | null; state: string }>
): ChatActivity {
  if (needs.some((n) => n.chatId === chatId)) return "needs";
  if (
    runs.some(
      (r) => r.chat_id === chatId && (r.state === "running" || r.state === "waiting_approval")
    )
  ) {
    return "running";
  }
  return null;
}

/** Which bot dialog a mock review URL (`?screen=`) opens; `null` for none. */
export type BotDialog = "edit" | "profile" | "skills" | "addSkill";

export function botDialogFromScreen(screen: string | null): BotDialog | null {
  switch (screen) {
    case "botform":
    case "botedit":
      return "edit";
    case "botprofile":
      return "profile";
    case "skills":
      return "skills";
    case "addskill":
      return "addSkill";
    default:
      return null;
  }
}
