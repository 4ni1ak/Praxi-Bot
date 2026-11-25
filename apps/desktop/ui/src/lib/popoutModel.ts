/**
 * Pure models for pop-out chat and live-view windows: URL/query/label resolution, ID validation,
 * drag-out geometry and cross-window decision-conflict detection.  Kept free of Tauri/runtime
 * imports so Node unit tests run directly against it.
 */

export type PopoutKind = "chat" | "live";
export type PopoutLiveSource = "home" | "local";
export type AppWindowKind = "main" | "overlay" | "widget" | "quick-ask" | "chat" | "live";

export type ResolvedWindowRoute =
  | { kind: "main" }
  | { kind: "overlay" }
  | { kind: "widget" }
  | { kind: "quick-ask" }
  | { kind: "chat"; chatId: string; computerId?: string; pinned: boolean }
  | {
      kind: "live";
      source: PopoutLiveSource;
      botId?: string;
      targetId?: string;
      computerId?: string;
      pinned: boolean;
    };

export interface PopoutChatTarget {
  kind: "chat";
  chatId: string;
  computerId?: string;
  title?: string;
  nearCursor?: boolean;
  windowFrame?: boolean;
}

export interface PopoutLiveTarget {
  kind: "live";
  liveSource?: PopoutLiveSource;
  botId?: string;
  targetId?: string;
  computerId?: string;
  title?: string;
  nearCursor?: boolean;
  windowFrame?: boolean;
}

export type PopoutTarget = PopoutChatTarget | PopoutLiveTarget;

const SAFE_ID_RE = /^[A-Za-z0-9_.:-]{1,128}$/;

export function isValidPopoutId(raw: string | null | undefined): raw is string {
  return typeof raw === "string" && SAFE_ID_RE.test(raw);
}

function cleanOptionalId(raw: string | null | undefined): string | undefined {
  if (typeof raw !== "string") return undefined;
  const trimmed = raw.trim();
  return isValidPopoutId(trimmed) ? trimmed : undefined;
}

function parseBoolParam(raw: string | null): boolean {
  return raw === "1" || raw === "true";
}

export function chatPopoutLabel(chatId: string): string | null {
  return isValidPopoutId(chatId) ? `chat-${chatId}` : null;
}

export function livePopoutLabel(source: PopoutLiveSource = "home"): string {
  return `live-${source}`;
}

export function popoutKindFromLabel(label: string | null | undefined): PopoutKind | null {
  if (!label) return null;
  if (label.startsWith("chat-") && isValidPopoutId(label.slice("chat-".length))) {
    return "chat";
  }
  if (label === "live-home" || label === "live-local") {
    return "live";
  }
  return null;
}

export function isPopoutLabel(label: string | null | undefined): boolean {
  return popoutKindFromLabel(label) !== null;
}

/**
 * Determine which window view should mount given `location.search`, `location.hash` and optional
 * Tauri window `label`.
 */
export function resolveWindowRoute(
  search: string,
  hash = "",
  label?: string | null
): ResolvedWindowRoute {
  const params = new URLSearchParams(search);
  const queryKind = params.get("window");

  if (queryKind === "overlay" || hash === "#overlay") return { kind: "overlay" };
  if (queryKind === "widget" || hash === "#widget") return { kind: "widget" };
  if (queryKind === "quick-ask") return { kind: "quick-ask" };

  if (queryKind === "chat" || (label && label.startsWith("chat-"))) {
    const rawChatId =
      params.get("chatId") ??
      params.get("id") ??
      (label?.startsWith("chat-") ? label.slice("chat-".length) : null);
    if (isValidPopoutId(rawChatId)) {
      return {
        kind: "chat",
        chatId: rawChatId,
        computerId: cleanOptionalId(params.get("computerId")),
        pinned: parseBoolParam(params.get("pinned"))
      };
    }
  }

  if (queryKind === "live" || label === "live-home" || label === "live-local") {
    const rawSource = params.get("source") ?? (label === "live-local" ? "local" : "home");
    const source: PopoutLiveSource = rawSource === "local" ? "local" : "home";
    return {
      kind: "live",
      source,
      botId: cleanOptionalId(params.get("botId")),
      targetId: cleanOptionalId(params.get("targetId")),
      computerId: cleanOptionalId(params.get("computerId")),
      pinned: parseBoolParam(params.get("pinned"))
    };
  }

  if (label?.startsWith("overlay")) return { kind: "overlay" };
  if (label === "widget") return { kind: "widget" };
  if (label === "quick-ask") return { kind: "quick-ask" };
  return { kind: "main" };
}

/** Build query string parameters for opening a pop-out window in mock/browser mode. */
export function buildPopoutSearchParams(target: PopoutTarget, mock = false): string {
  const params = new URLSearchParams();
  if (mock) params.set("mock", "1");
  if (target.kind === "chat") {
    params.set("window", "chat");
    params.set("chatId", target.chatId);
    if (target.computerId) params.set("computerId", target.computerId);
  } else {
    params.set("window", "live");
    params.set("source", target.liveSource ?? "home");
    if (target.botId) params.set("botId", target.botId);
    if (target.targetId) params.set("targetId", target.targetId);
    if (target.computerId) params.set("computerId", target.computerId);
  }
  return `?${params.toString()}`;
}

export interface Point2D {
  x: number;
  y: number;
}

export interface ViewportSize {
  width: number;
  height: number;
}

export interface RectBounds {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Whether a viewport pointer position has crossed outside the window bounds. */
export function isDragOutsideWindow(
  point: Point2D,
  viewport: ViewportSize,
  margin = 8
): boolean {
  return (
    point.x < -margin ||
    point.y < -margin ||
    point.x > viewport.width + margin ||
    point.y > viewport.height + margin
  );
}

/** Euclidean distance from a point to a source element rectangle (0 if inside). */
export function distanceFromRect(point: Point2D, rect: RectBounds): number {
  const dx = point.x < rect.left ? rect.left - point.x : point.x > rect.right ? point.x - rect.right : 0;
  const dy = point.y < rect.top ? rect.top - point.y : point.y > rect.bottom ? point.y - rect.bottom : 0;
  return Math.hypot(dx, dy);
}

/**
 * Decide whether a completed pointer drag should pop the target out into its own window:
 * requires at least `minDistance` pointer movement AND either leaving the window viewport
 * or pulling at least `escapeDistance` pixels away from the source element's bounding box.
 */
export function shouldPopOutOnDrag({
  start,
  current,
  viewport,
  sourceRect,
  minDistance = 12,
  escapeDistance = 96,
  edgeMargin = 8,
  leftWindow = false
}: {
  start: Point2D;
  current: Point2D;
  viewport: ViewportSize;
  sourceRect?: RectBounds | null;
  minDistance?: number;
  escapeDistance?: number;
  edgeMargin?: number;
  leftWindow?: boolean;
}): boolean {
  const moved = Math.hypot(current.x - start.x, current.y - start.y);
  if (moved < minDistance) return false;
  if (leftWindow) return true;
  if (isDragOutsideWindow(current, viewport, edgeMargin)) return true;
  if (sourceRect && distanceFromRect(current, sourceRect) >= escapeDistance) return true;
  return false;
}

/**
 * Identify daemon errors indicating an approval, question or secret prompt was already decided
 * (for example in another pop-out window or the main window).
 */
export function isAlreadyDecidedError(err: unknown): boolean {
  if (!err || typeof err !== "object") return false;
  const status = typeof (err as { status?: unknown }).status === "number"
    ? (err as { status: number }).status
    : undefined;
  const code = typeof (err as { code?: unknown }).code === "string"
    ? (err as { code: string }).code.toLowerCase()
    : "";
  const message = typeof (err as { message?: unknown }).message === "string"
    ? (err as { message: string }).message
    : err instanceof Error
      ? err.message
      : "";

  if (/no route|method not allowed/i.test(message)) return false;

  if (
    code === "already_decided" ||
    code === "already_answered"
  ) {
    return true;
  }

  if (status === 404 || code === "not_found") {
    return (
      /(pending approval|pending question|secret request|already (decided|answered)|question.*(closed|expired))/i.test(
        message
      )
    );
  }

  if (status === 410 || code === "gone") return /(expired|closed|approval|question|secret)/i.test(message);
  return /(already\s+(decided|answered|resolved)|pending (approval|question).*not found)/i.test(
    message
  );
}
