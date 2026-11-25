import { invoke } from "@tauri-apps/api/core";
import { MOCK_BASE_URL } from "../api/mockMode";

import { isLocalBaseUrl } from "./liveViewState";
import { isPopoutWindow, popoutOwnerKey } from "./popout";

let cached: Promise<string | undefined> | null = null;

/**
 * The owner key only the desktop app's main window or pop-out windows can read (`owner_key` /
 * `popout_owner_key` commands): it proves a request comes from the owner at the keyboard.
 */
export function ownerKey(baseUrl: string | undefined): Promise<string | undefined> {
  if (baseUrl === MOCK_BASE_URL) return Promise.resolve("mock");
  if (!baseUrl || !isLocalBaseUrl(baseUrl)) return Promise.resolve(undefined);
  cached ??= (isPopoutWindow() ? popoutOwnerKey() : invoke<string>("owner_key")).catch(() => {
    // The daemon may still be starting. A failed read must be retryable after
    // reconnecting, including when a voice call's preflight asks again.
    cached = null;
    return undefined;
  });
  return cached;
}
