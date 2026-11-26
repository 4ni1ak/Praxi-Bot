import { isMockMode } from "../api/mockMode.ts";
import { noScreenDetailKey } from "./computerLabel.ts";

/** The exact command that adds the desktop session to a Linux server (run on the server). */
export const DESKTOP_SESSION_COMMAND = "sudo ./install-linux.sh --update --with-desktop-session";

/** The command that grants this computer's screen view to every desktop (run on the server). */
export const SCREEN_GRANT_COMMAND = "bot host screen --all-desktops";

/** Target-level reasons that say nothing about the cause: the host's own note says more. */
const GENERIC_REASONS = new Set(["unavailable", "unsupported"]);

/** Codes after which the cure is a desktop session on the server. */
const NEEDS_SESSION = new Set(["no_display", "desktop_off"]);

const FIX_KEY: Record<string, string> = {
  desktop_off: "addSession",
  no_display: "addSession",
  wayland: "wayland",
  screen_unavailable: "locked",
  start_failed: "restart",
  starting: "wait",
  timed_out: "wait",
  unsupported_platform: "platform"
};

/** Where the screen lives: this very PC (a Windows/macOS/Linux desktop) or a remote server. */
export interface ScreenPlace {
  /** The daemon runs on this computer (the viewer's own machine), not on a server. */
  local: boolean;
  /** This computer's system, when it is local. */
  os: "windows" | "macos" | "linux" | "";
}

/** The viewer's own system, read from the browser/webview user agent. */
export function thisOs(ua: string = typeof navigator === "undefined" ? "" : navigator.userAgent): ScreenPlace["os"] {
  return /Windows/i.test(ua) ? "windows" : /Mac/i.test(ua) ? "macos" : /Linux|X11/i.test(ua) ? "linux" : "";
}

/**
 * Where the screen being looked at lives. `isLocalClient` is "the daemon is on this computer".
 * Review state (mock mode only): `?lvPlace=host` shows the server wording.
 */
export function screenPlace(isLocalClient: boolean): ScreenPlace {
  const forcedHost =
    isMockMode() && typeof window !== "undefined" && new URLSearchParams(window.location.search).get("lvPlace") === "host";
  return { local: isLocalClient && !forcedHost, os: thisOs() };
}

/** The target-level reason / start failure words, folded into the host's one vocabulary of codes. */
function codeOf(reason: string | null, noScreen: string | null): string {
  if (noScreen) return noScreen;
  if (reason === "timeout" || reason === "timed_out") return "timed_out";
  return "unknown";
}

/** What a local PC (never a server) should do for each code. The Linux install command is NOT one of them. */
function localFix(code: string, os: ScreenPlace["os"]): string {
  switch (code) {
    case "desktop_off":
      return "liveView.noScreenFix.localOff";
    case "no_display":
      return "liveView.noScreenFix.localNoDisplay";
    case "unsupported_platform":
      return "liveView.noScreenFix.localPlatform";
    case "wayland":
      return "liveView.noScreenFix.localWayland";
    case "starting":
      return "liveView.noScreenFix.localWait";
    default:
      return os === "macos"
        ? "liveView.noScreenFix.localMac"
        : os === "linux"
          ? "liveView.noScreenFix.localLinux"
          : "liveView.noScreenFix.localWindows";
  }
}

export interface ScreenExplanation {
  /** i18n key of the sentence that says what is wrong. */
  detailKey: string;
  /** i18n key of the sentence that says what to do. */
  fixKey: string;
  /** A command for the server, when one cures it (never for this computer). */
  command: string | null;
  /** An extra sentence (this PC: where the bot's own browser can be watched), or null. */
  extraKey: string | null;
}

/**
 * The real reason a screen cannot be shown and how to fix it. `reason` is the target's own typed
 * reason (or the card's start failure), `noScreen` the host's "why there is no screen" code. One
 * vocabulary for every spelling of "the screen did not start": the same detail whatever raised it.
 * `place` decides the cure: the Linux install command belongs to a server only; this PC gets what to
 * allow, and where the bot's browser can be watched. Returns `null` when the target's reason is
 * specific (the viewer words it itself) and the host has nothing more to say.
 */
export function explainScreen(
  reason: string | null,
  noScreen: string | null,
  place: ScreenPlace = { local: false, os: "" }
): ScreenExplanation | null {
  const timedOut = reason === "timeout" || reason === "timed_out";
  if (!noScreen && !timedOut && reason !== null && !GENERIC_REASONS.has(reason)) {
    return null;
  }
  const code = codeOf(reason, noScreen);
  if (place.local) {
    return {
      detailKey: noScreenDetailKey(code),
      fixKey: localFix(code, place.os),
      command: null,
      extraKey: "liveView.noScreenFix.browserOwnWindow"
    };
  }
  const known = noScreen !== null || timedOut;
  return {
    detailKey: noScreenDetailKey(code),
    fixKey: `liveView.noScreenFix.${known ? (FIX_KEY[code] ?? "generic") : "generic"}`,
    command: NEEDS_SESSION.has(code) || !known ? DESKTOP_SESSION_COMMAND : null,
    extraKey: null
  };
}
