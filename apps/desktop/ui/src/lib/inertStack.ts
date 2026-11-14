/**
 * Ref-counted "the app behind a dialog is inert" bookkeeping. Every dialog that
 * wants `#root` inert acquires a token and releases it on close; `#root` is
 * inert exactly while at least one token is held, whatever the open/close order
 * (a dialog's cleanup must never look at other dialogs in the DOM).
 */
export interface InertTracker {
  acquire(token: number): void;
  release(token: number): void;
  readonly holders: number;
}

export function createInertTracker(setInert: (inert: boolean) => void): InertTracker {
  const held = new Set<number>();
  return {
    acquire(token) {
      held.add(token);
      setInert(true);
    },
    release(token) {
      held.delete(token);
      setInert(held.size > 0);
    },
    get holders() {
      return held.size;
    }
  };
}

/** The shared tracker for `#root` (a no-op outside a browser). */
export const appInert: InertTracker = createInertTracker((inert) => {
  if (typeof document === "undefined") {
    return;
  }
  const root = document.getElementById("root");
  if (root) {
    root.inert = inert;
  }
});
