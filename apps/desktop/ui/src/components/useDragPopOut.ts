import { useEffect, useRef, useState, type PointerEvent } from "react";
import { shouldPopOutOnDrag, type Point2D, type RectBounds } from "../lib/popoutModel";

export interface UseDragPopOutOptions {
  /** Called when the pointer drag crosses outside the window or far from the source element. */
  onPopOut?: (opts: { nearCursor: true }) => void;
  /** CSS selector of inner interactive children whose pointer presses should stay normal clicks. */
  ignoreSelector?: string;
}

/**
 * Pointer-based drag-out gesture (HTML5 DnD is unreliable in WebView2 / WKWebView).
 * Short clicks (<12px movement) pass through untouched; dragging >=12px and either leaving the
 * window bounds or pulling far away from the source element suppresses the trailing click and
 * triggers `onPopOut`.
 */
export function useDragPopOut({ onPopOut, ignoreSelector }: UseDragPopOutOptions) {
  const [dragging, setDragging] = useState(false);
  const cleanupRef = useRef<(() => void) | null>(null);
  const suppressRef = useRef(false);
  const liveRef = useRef({ onPopOut, ignoreSelector });
  liveRef.current = { onPopOut, ignoreSelector };

  useEffect(() => () => cleanupRef.current?.(), []);

  const onPointerDown = (e: PointerEvent<HTMLElement>) => {
    if (e.button !== 0 || e.pointerType === "touch" || !liveRef.current.onPopOut) return;
    if (liveRef.current.ignoreSelector && (e.target as HTMLElement | null)?.closest(liveRef.current.ignoreSelector)) {
      return;
    }

    cleanupRef.current?.();
    const targetEl = e.currentTarget;
    const sourceRect: RectBounds = targetEl.getBoundingClientRect();
    const pointerId = e.pointerId;
    const start: Point2D = { x: e.clientX, y: e.clientY };
    let active = false;
    let triggered = false;

    const finish = (popOutPoint: Point2D | null, leftWindow = false) => {
      cleanupRef.current?.();
      cleanupRef.current = null;
      setDragging(false);
      if (active) {
        suppressRef.current = true;
        window.setTimeout(() => {
          suppressRef.current = false;
        }, 50);
      }
      if (!triggered && popOutPoint && liveRef.current.onPopOut) {
        const viewport = { width: window.innerWidth, height: window.innerHeight };
        if (shouldPopOutOnDrag({ start, current: popOutPoint, viewport, sourceRect, leftWindow })) {
          triggered = true;
          liveRef.current.onPopOut({ nearCursor: true });
        }
      }
    };

    const onMove = (ev: globalThis.PointerEvent) => {
      const current: Point2D = { x: ev.clientX, y: ev.clientY };
      if (!active && Math.hypot(current.x - start.x, current.y - start.y) < 12) {
        return;
      }
      if (!active) {
        // Capture after a real drag, so a short press still clicks the original button.
        targetEl.setPointerCapture(pointerId);
        active = true;
        setDragging(true);
      }
      const viewport = { width: window.innerWidth, height: window.innerHeight };
      if (
        current.x < -8 ||
        current.y < -8 ||
        current.x > viewport.width + 8 ||
        current.y > viewport.height + 8
      ) {
        finish(current);
      }
    };

    const onMouseLeave = (ev: MouseEvent) => {
      if (!active) return;
      if (!ev.relatedTarget) {
        finish({ x: ev.clientX, y: ev.clientY }, true);
      }
    };

    const onUp = (ev: globalThis.PointerEvent) => {
      finish(active ? { x: ev.clientX, y: ev.clientY } : null);
    };

    const onKeyDown = (ev: globalThis.KeyboardEvent) => {
      if (ev.key === "Escape") {
        finish(null);
      }
    };

    const onCancel = () => finish(null);

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp, { once: true });
    window.addEventListener("pointercancel", onCancel, { once: true });
    document.documentElement.addEventListener("mouseleave", onMouseLeave);
    window.addEventListener("keydown", onKeyDown);

    cleanupRef.current = () => {
      if (targetEl.hasPointerCapture(pointerId)) targetEl.releasePointerCapture(pointerId);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      document.documentElement.removeEventListener("mouseleave", onMouseLeave);
      window.removeEventListener("keydown", onKeyDown);
    };
  };

  const onClickCapture = (e: { preventDefault: () => void; stopPropagation: () => void }) => {
    if (!suppressRef.current) return;
    suppressRef.current = false;
    e.preventDefault();
    e.stopPropagation();
  };

  return {
    dragging,
    dragProps: onPopOut
      ? {
          onPointerDown,
          onClickCapture,
          "data-dragging-popout": dragging ? ("true" as const) : undefined
        }
      : {}
  };
}
