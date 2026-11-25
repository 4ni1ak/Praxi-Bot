/**
 * The chat inside the full-screen live view has three states and the owner moves between them:
 *
 * - `closed`: only a "Chat" button at the bottom-right; the screen share has the whole window.
 * - `small`: a small chat card over the bottom-right (the latest messages and the composer).
 * - `large`: a chat column on the right with the whole history; the screen share stays on the left.
 *
 * The last state is remembered per bot (a per-viewer convenience kept in local storage).
 */
export type ChatDockMode = "closed" | "small" | "large";
export type ChatDockAction = "open" | "expand" | "collapse" | "close";

export const CHAT_DOCK_MODES: readonly ChatDockMode[] = ["closed", "small", "large"];

/** Where a button or key takes the chat from `mode`. Impossible moves leave it where it is. */
export function nextChatMode(mode: ChatDockMode, action: ChatDockAction): ChatDockMode {
  switch (action) {
    case "open":
      return mode === "closed" ? "small" : mode;
    case "expand":
      return mode === "closed" ? mode : "large";
    case "collapse":
      return mode === "large" ? "small" : mode;
    case "close":
      return "closed";
  }
}

/** Escape steps back one level (large -> small -> closed); it never skips a level. */
export function escapeChatMode(mode: ChatDockMode): ChatDockMode {
  return mode === "large" ? "small" : "closed";
}

const KEY_PREFIX = "praxi.liveView.chatMode.";

function safeStorage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function isMode(value: unknown): value is ChatDockMode {
  return typeof value === "string" && (CHAT_DOCK_MODES as readonly string[]).includes(value);
}

/** The state this bot's chat was left in; closed when nothing (valid) is stored or storage is blocked. */
export function loadChatMode(
  botId: string | null | undefined,
  storage: Pick<Storage, "getItem"> | null = safeStorage()
): ChatDockMode {
  if (!botId) {
    return "closed";
  }
  try {
    const value = storage?.getItem(KEY_PREFIX + botId);
    return isMode(value) ? value : "closed";
  } catch {
    return "closed";
  }
}

export function saveChatMode(
  botId: string | null | undefined,
  mode: ChatDockMode,
  storage: Pick<Storage, "setItem"> | null = safeStorage()
): void {
  if (!botId) {
    return;
  }
  try {
    storage?.setItem(KEY_PREFIX + botId, mode);
  } catch {
    // Blocked storage: the choice just lasts for this session.
  }
}
