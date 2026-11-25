import { invoke } from "@tauri-apps/api/core";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "ui/tokens.css";
import "@fontsource-variable/geist/wght.css";
import "@fontsource-variable/geist-mono/wght.css";
import "@fontsource/opendyslexic/400.css";
import "@fontsource/opendyslexic/700.css";
import { ApiClient } from "./api/client";
import { App } from "./App";
import "./global.css";
import "./i18n";
import { currentWindowLabel, popoutOwnerKey, resolveWindowRoute } from "./lib/popout";
import { isTauriRuntime } from "./overlay/events";
import { OverlayWindow } from "./overlay/OverlayWindow";
import { PopoutWindow } from "./popout/PopoutWindow";
import { QuickAskWindow } from "./quickask/QuickAskWindow";
import { WidgetWindow } from "./overlay/WidgetWindow";
import { paintSavedAppearanceEarly } from "./state/useApplyAppearance";

const resolvedRoute =
  typeof window !== "undefined"
    ? resolveWindowRoute(window.location.search, window.location.hash, currentWindowLabel())
    : ({ kind: "main" } as const);
const windowKind = resolvedRoute.kind;
document.documentElement.dataset.windowKind = windowKind;

// The owner key of this app's daemon (`main` and pop-out windows only; the overlay, widget and
// Quick ask windows get none). Requested once and kept in memory.
let ownerKey: Promise<string | null> | null = null;
if (isTauriRuntime() && windowKind === "main") {
  ApiClient.ownerKeyProvider = () => (ownerKey ??= invoke<string>("owner_key").catch(() => null));
} else if (isTauriRuntime() && (windowKind === "chat" || windowKind === "live")) {
  ApiClient.ownerKeyProvider = () => (ownerKey ??= popoutOwnerKey().catch(() => null));
}

paintSavedAppearanceEarly(windowKind !== "main");

// Layout probe: same guard as state/useQuotas.ts, so production builds drop it.
if (
  (import.meta.env.DEV || import.meta.env.VITE_MOCK === "1") &&
  ["layout", "search", "content"].includes(new URLSearchParams(window.location.search).get("probe") ?? "")
) {
  const stored = new URLSearchParams(window.location.search).get("stored");
  if (stored) {
    // A layout saved by an older session, read by App's first render.
    try {
      window.localStorage.setItem("praxi.shell.layout.v1", JSON.stringify({ rail: 264, panel: Number(stored) }));
    } catch {
      // Storage unavailable: the probe then runs with the defaults.
    }
  }
  void import("./dev/layoutProbe")
    .then((m) => {
        const mode = new URLSearchParams(window.location.search).get("probe");
        return mode === "search" ? m.runSearchProbe() : mode === "content" ? m.runContentProbe() : m.runLayoutProbe();
      })
    .catch((e: unknown) => {
      document.documentElement.dataset.probeError = String(e);
    });
}

const container = document.getElementById("root");
if (!container) {
  throw new Error("Root container #root not found");
}

const root = createRoot(container);

// Dev/mock-only avatar review gallery (`?mock=1&gallery=avatars`); dropped from production builds.
const showAvatarGallery =
  (import.meta.env.DEV || import.meta.env.VITE_MOCK === "1") &&
  new URLSearchParams(window.location.search).get("gallery") === "avatars";

if (showAvatarGallery) {
  void import("./dev/AvatarGallery").then(({ AvatarGallery }) => {
    root.render(
      <StrictMode>
        <AvatarGallery />
      </StrictMode>
    );
  });
} else {
  root.render(
    <StrictMode>
      {resolvedRoute.kind === "overlay" ? (
        <OverlayWindow />
      ) : resolvedRoute.kind === "widget" ? (
        <WidgetWindow />
      ) : resolvedRoute.kind === "quick-ask" ? (
        <QuickAskWindow />
      ) : resolvedRoute.kind === "chat" || resolvedRoute.kind === "live" ? (
        <PopoutWindow route={resolvedRoute} />
      ) : (
        <App />
      )}
    </StrictMode>
  );
}

