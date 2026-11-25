import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { isMockMode } from "../api/mockMode";
import type { Connection } from "../api/connection";
import { isTauriRuntime } from "../overlay/events";
import {
  buildPopoutSearchParams,
  chatPopoutLabel,
  isPopoutLabel,
  livePopoutLabel,
  popoutKindFromLabel,
  resolveWindowRoute,
  type PopoutChatTarget,
  type PopoutLiveTarget,
  type PopoutTarget,
  type ResolvedWindowRoute
} from "./popoutModel";

export * from "./popoutModel";

export const EVENT_LIVE_TARGET = "popout:live_target";

export interface LiveTargetUpdatePayload {
  botId?: string | null;
  targetId?: string | null;
  computerId?: string | null;
}

/** Current Tauri window label when running inside the desktop shell, otherwise null. */
export function currentWindowLabel(): string | null {
  if (!isTauriRuntime()) return null;
  try {
    return getCurrentWebviewWindow().label;
  } catch {
    return null;
  }
}

/** Resolve the current browser/Tauri window route (cached safely at call time). */
export function currentWindowRoute(): ResolvedWindowRoute {
  if (typeof window === "undefined") return { kind: "main" };
  return resolveWindowRoute(
    window.location.search,
    window.location.hash,
    currentWindowLabel()
  );
}

/** Whether this execution context is a chat or live pop-out window. */
export function isPopoutWindow(): boolean {
  const route = currentWindowRoute();
  return route.kind === "chat" || route.kind === "live" || isPopoutLabel(currentWindowLabel());
}

/** Open or focus a pop-out window from the main window. */
export async function openPopoutWindow(target: PopoutTarget): Promise<string | null> {
  const label =
    target.kind === "chat"
      ? chatPopoutLabel(target.chatId)
      : livePopoutLabel(target.liveSource ?? "home");
  if (!label) throw new Error("invalid pop-out chat id");

  if (!isTauriRuntime()) {
    if (typeof window !== "undefined") {
      const search = buildPopoutSearchParams(target, isMockMode());
      const url = `${window.location.pathname}${search}`;
      const features =
        target.kind === "chat"
          ? "width=620,height=780,resizable=yes"
          : "width=980,height=680,resizable=yes";
      const opened = window.open(url, label, features);
      if (!opened) throw new Error("the new window could not be opened");
      opened.focus();
    }
    return label;
  }

  const kind = target.kind;
  const payload = {
    request: {
      kind,
      chatId: target.kind === "chat" ? target.chatId : null,
      liveSource: target.kind === "live" ? (target.liveSource ?? "home") : null,
      botId: target.kind === "live" ? (target.botId ?? null) : null,
      targetId: target.kind === "live" ? (target.targetId ?? null) : null,
      computerId: target.computerId ?? null,
      title: target.title ?? null,
      nearCursor: target.nearCursor ?? false,
      windowFrame: target.windowFrame ?? null
    }
  };
  return invoke<string>("open_popout_window", payload);
}

export function openChatPopout(
  chatId: string,
  opts: Omit<PopoutChatTarget, "kind" | "chatId"> = {}
): Promise<string | null> {
  return openPopoutWindow({ kind: "chat", chatId, ...opts });
}

export function openLivePopout(
  opts: Omit<PopoutLiveTarget, "kind"> = {}
): Promise<string | null> {
  return openPopoutWindow({ kind: "live", ...opts });
}

/** Daemon connection granted exclusively to `chat-*` and `live-*` pop-out windows. */
export function popoutConnect(): Promise<Connection> {
  return invoke<Connection>("popout_connect");
}

/** Owner key granted exclusively to `chat-*` and `live-*` pop-out windows. */
export function popoutOwnerKey(): Promise<string> {
  return invoke<string>("popout_owner_key");
}

/** Persist and apply always-on-top state for the calling pop-out window. */
export function popoutSetAlwaysOnTop(pinned: boolean): Promise<boolean> {
  const payload = { pinned };
  return invoke<boolean>("popout_set_always_on_top", payload);
}

/** Bring the main desktop window back into focus from a pop-out window. */
export function popoutFocusMain(): Promise<void> {
  return invoke("popout_focus_main");
}

/** Subscribe to retargeting events sent when `openPopoutWindow` focuses an existing live pop-out. */
export function onLiveTargetUpdate(
  handler: (payload: LiveTargetUpdatePayload) => void
): () => void {
  if (!isTauriRuntime() || popoutKindFromLabel(currentWindowLabel()) !== "live") {
    return () => {};
  }
  let stopped = false;
  let unlisten: UnlistenFn | null = null;
  void listen<LiveTargetUpdatePayload>(EVENT_LIVE_TARGET, (event) => {
    handler(event.payload);
  }).then((fn) => {
    if (stopped) fn();
    else unlisten = fn;
  }).catch(() => {});
  return () => {
    stopped = true;
    unlisten?.();
  };
}
