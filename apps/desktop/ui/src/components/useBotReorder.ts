import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { moveBotBy, moveBotTo } from "../lib/botOrder";

const THRESHOLD = 6;

export interface BotDrag {
  id: string;
  from: number;
  over: number;
}

/**
 * Pointer + keyboard reordering for the sidebar bot rows (no drag library; HTML5 drag-and-drop is
 * unreliable inside the Tauri webview).  Rows carry `data-bot-id`.  A press that moves more than a
 * few pixels becomes a drag and swallows the click that follows; the grip button reorders with
 * the arrow keys.  `onCommit` receives the complete new id order.
 */
export function useBotReorder(ids: readonly string[], onCommit: (next: string[]) => void, onMoved?: (id: string, pos: number) => void) {
  const [drag, setDrag] = useState<BotDrag | null>(null);
  const root = useRef<HTMLElement | null>(null);
  const press = useRef<{ id: string; y: number; active: boolean } | null>(null);
  const suppress = useRef(false);
  const cleanup = useRef<(() => void) | null>(null);
  const live = useRef({ ids, onCommit, onMoved });
  live.current = { ids, onCommit, onMoved };

  useEffect(() => () => cleanup.current?.(), []);

  const rowAt = (y: number): number => {
    const rows = [...(root.current?.querySelectorAll<HTMLElement>("[data-bot-id]") ?? [])];
    let best = rows.length - 1;
    for (let i = 0; i < rows.length; i++) {
      if (y < rows[i].getBoundingClientRect().bottom) {
        best = i;
        break;
      }
    }
    return Math.max(0, best);
  };

  const begin = (id: string, e: PointerEvent) => {
    if (e.button !== 0 || live.current.ids.length < 2) return;
    press.current = { id, y: e.clientY, active: false };
    const move = (ev: globalThis.PointerEvent) => {
      const p = press.current;
      if (!p) return;
      if (!p.active && Math.abs(ev.clientY - p.y) < THRESHOLD) return;
      p.active = true;
      setDrag({ id: p.id, from: live.current.ids.indexOf(p.id), over: rowAt(ev.clientY) });
    };
    const finish = (commit: boolean) => {
      const p = press.current;
      press.current = null;
      cleanup.current?.();
      cleanup.current = null;
      if (!p?.active) return;
      suppress.current = true;
      setTimeout(() => (suppress.current = false), 0);
      setDrag((d) => {
        if (commit && d && d.over !== d.from) {
          const next = moveBotTo(live.current.ids, d.id, d.over);
          live.current.onCommit(next);
          live.current.onMoved?.(d.id, next.indexOf(d.id) + 1);
        }
        return null;
      });
    };
    const up = () => finish(true);
    const key = (ev: globalThis.KeyboardEvent) => {
      if (ev.key === "Escape") finish(false);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    window.addEventListener("keydown", key);
    cleanup.current = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      window.removeEventListener("keydown", key);
    };
  };

  const onKeyDown = useCallback((id: string, e: KeyboardEvent) => {
    const delta = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0;
    if (!delta) return;
    e.preventDefault();
    const { ids: cur, onCommit: commit, onMoved: moved } = live.current;
    const next = moveBotBy(cur, id, delta);
    if (next.some((x, i) => x !== cur[i])) {
      commit(next);
      moved?.(id, next.indexOf(id) + 1);
      // The row re-renders in its new place; keep the grip focused for the next step.
      requestAnimationFrame(() =>
        root.current?.querySelector<HTMLElement>(`[data-bot-id="${CSS.escape(id)}"] [data-grip]`)?.focus()
      );
    }
  }, []);

  return {
    drag,
    containerRef: (el: HTMLElement | null) => {
      root.current = el;
    },
    rowProps: (id: string) => ({
      "data-bot-id": id,
      onPointerDown: (e: PointerEvent) => begin(id, e),
      onClickCapture: (e: { preventDefault(): void; stopPropagation(): void }) => {
        if (suppress.current) {
          e.preventDefault();
          e.stopPropagation();
        }
      }
    }),
    gripProps: (id: string) => ({
      "data-grip": "",
      onKeyDown: (e: KeyboardEvent) => onKeyDown(id, e)
    })
  };
}
