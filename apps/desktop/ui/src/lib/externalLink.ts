import { invoke } from "@tauri-apps/api/core";
import { isComposioLink } from "./composioApps";
import { sanitizeUrl } from "./markdown";

/**
 * Open a link in the system browser, never inside the app window. Only
 * http(s) / mailto pass `sanitizeUrl`; anything else is ignored.
 */
export function openExternal(href: string): boolean {
  const url = sanitizeUrl(href);
  if (!url || typeof window === "undefined") {
    return false;
  }
  window.open(url, "_blank", "noopener,noreferrer");
  return true;
}

/** Open a Composio link (connect page, key page) in the system browser; any other URL is refused. */
export async function openComposioLink(href: string, dashboard = false): Promise<boolean> {
  if (!isComposioLink(href, dashboard)) return false;
  if (typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window)) {
    // Plain browser (dev server, design review): the page opens in a new tab.
    window.open(href, "_blank", "noopener,noreferrer");
    return true;
  }
  try {
    await invoke("open_composio_link", { href, dashboard });
    return true;
  } catch {
    return false;
  }
}
