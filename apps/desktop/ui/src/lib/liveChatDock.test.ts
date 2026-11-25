import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  escapeChatMode,
  loadChatMode,
  nextChatMode,
  saveChatMode,
  type ChatDockMode
} from "./liveChatDock.ts";

const memory = () => {
  const store = new Map<string, string>();
  return {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v)
  };
};

test("the Chat button opens the small chat; expand makes the column; collapse and close step back", () => {
  let mode: ChatDockMode = "closed";
  mode = nextChatMode(mode, "open");
  assert.equal(mode, "small");
  mode = nextChatMode(mode, "expand");
  assert.equal(mode, "large");
  mode = nextChatMode(mode, "collapse");
  assert.equal(mode, "small");
  mode = nextChatMode(mode, "close");
  assert.equal(mode, "closed");
  // A big chat can also be closed straight to the button.
  assert.equal(nextChatMode("large", "close"), "closed");
});

test("buttons that do not apply leave the chat where it is", () => {
  assert.equal(nextChatMode("closed", "expand"), "closed");
  assert.equal(nextChatMode("closed", "collapse"), "closed");
  assert.equal(nextChatMode("small", "open"), "small");
  assert.equal(nextChatMode("large", "open"), "large");
  assert.equal(nextChatMode("small", "collapse"), "small");
});

test("Escape in the chat steps back one level and never skips one", () => {
  assert.equal(escapeChatMode("large"), "small");
  assert.equal(escapeChatMode("small"), "closed");
  assert.equal(escapeChatMode("closed"), "closed");
});

test("each bot remembers its own chat state; the default is the closed button", () => {
  const storage = memory();
  assert.equal(loadChatMode("bot-a", storage), "closed");
  saveChatMode("bot-a", "large", storage);
  saveChatMode("bot-b", "small", storage);
  assert.equal(loadChatMode("bot-a", storage), "large");
  assert.equal(loadChatMode("bot-b", storage), "small");
  assert.equal(loadChatMode("bot-c", storage), "closed");
  assert.equal(loadChatMode(null, storage), "closed");
  storage.setItem("praxi.liveView.chatMode.bot-a", "bogus");
  assert.equal(loadChatMode("bot-a", storage), "closed");
});

test("blocked storage never throws", () => {
  const broken = {
    getItem: () => {
      throw new Error("blocked");
    },
    setItem: () => {
      throw new Error("blocked");
    }
  };
  assert.equal(loadChatMode("bot-a", broken), "closed");
  assert.doesNotThrow(() => saveChatMode("bot-a", "small", broken));
});

/**
 * Wiring guards: the pure rules above are worthless if the screens stop using them, and the layout
 * promises (screen share left and large, chat on the right, nothing resizes abruptly) live in CSS.
 */
const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
const viewer = read("../views/LiveViewer.tsx");
const viewerChat = read("../views/LiveViewerChat.tsx");
const viewerCss = read("../views/LiveViewer.module.css");
const chatView = read("../views/ChatView.tsx");
const app = read("../App.tsx");

test("the three chat states are rendered by the viewer and remembered per bot", () => {
  assert.match(viewer, /loadChatMode\(botId\)/);
  assert.match(viewer, /saveChatMode\(botId, next\)/);
  assert.match(viewerChat, /data-mode=\{shown\}/);
  for (const testId of [
    "live-viewer-chat-open",
    "live-viewer-chat-expand",
    "live-viewer-chat-collapse",
    "live-viewer-chat-close"
  ]) {
    assert.ok(viewerChat.includes(`data-testid="${testId}"`), testId);
  }
  assert.match(viewerChat, /onAction\("open"\)/);
  assert.match(viewerChat, /onAction\("expand"\)/);
  assert.match(viewerChat, /onAction\("collapse"\)/);
  assert.match(viewerChat, /onAction\("close"\)/);
});

test("the large chat is a column on the right and the screen column makes room with a transition", () => {
  assert.match(viewerCss, /\.chatCard\[data-mode="large"\]\s*\{[^}]*right:\s*var\(--space-0\)[^}]*height:\s*100%/s);
  assert.match(viewerCss, /\.body\[data-chat="large"\] \.mainCol\s*\{\s*padding-inline-end:\s*var\(--chat-w\)/);
  assert.match(viewerCss, /\.mainCol\s*\{[^}]*transition:\s*padding-inline-end var\(--motion-duration-normal\)/s);
  assert.match(viewerCss, /\.chatCard\s*\{[^}]*transition:[^}]*width var\(--motion-duration-normal\)[^}]*height var\(--motion-duration-normal\)/s);
  // No hard-coded radius or colour: the Appearance radius tokens and theme tokens only.
  const chatCss = viewerCss.slice(viewerCss.indexOf("Chat inside the live view"));
  assert.doesNotMatch(chatCss, /border-radius:\s*\d/);
  assert.doesNotMatch(chatCss, /#[0-9a-f]{3,8}\b/i);
});

test("the conversation is one chat view that portals into the card, with no second copy", () => {
  assert.match(chatView, /createPortal\(chatRoot, liveChatSlots\.root\)/);
  // The run status line with Stop is part of the composer in every state (it used to be hidden in the live view).
  assert.doesNotMatch(chatView, /!liveChatSlots\?\.composer/);
  assert.match(chatView, /\}, \[chat\?\.id, liveRoot\]\);/, "the scroll observer must follow the chat to its new DOM");
});

test("the bot panel stays open when the chat is clicked: no click-away, and the viewer yields to it", () => {
  assert.doesNotMatch(app, /addEventListener\("pointerdown"/);
  assert.doesNotMatch(app, /dismissOverlayPanel/);
  assert.match(app, /liveViewState: screenOpen \? "live" : "idle"/);
  assert.match(viewer, /data-panel=\{infoPanel\?\.open \? "open" : "closed"\}/);
  assert.match(viewerCss, /\.backdrop\[data-panel="open"\]\s*\{\s*right:/);
  assert.match(viewerCss, /\.backdrop\s*\{[^}]*transition:\s*right var\(--motion-duration-normal\)/s);
  // Escape closes the panel before the viewer.
  assert.match(viewer, /infoPanelRef\.current\?\.open/);
});
