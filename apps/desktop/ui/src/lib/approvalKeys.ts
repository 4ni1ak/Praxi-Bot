/**
 * Keyboard shortcuts of the approval dock, kept pure so they run without a browser:
 * Enter allows the card once, Escape denies it. They only apply while the keys have nothing else
 * to do: the focus is on the page itself, on the EMPTY composer, or inside the dock. A key typed
 * into any other field, into the composer once it holds text, or while a dialog is open never
 * decides a card, and Enter never replaces a click on the button that has the focus.
 */

const EDITABLE = "input, textarea, select, [contenteditable=''], [contenteditable='true'], [contenteditable='plaintext-only']";
const INTERACTIVE =
  "button, a[href], summary, [role='button'], [role='menuitem'], [role='tab'], [role='option'], [role='checkbox'], [role='switch'], [role='radio']";
const DIALOG = "[role='dialog'], [aria-modal='true'], dialog";

/** The slice of a DOM element these rules need. */
export interface KeyTargetLike {
  closest(selector: string): unknown;
  value?: unknown;
  tagName?: string;
}

export interface ApprovalKeyEvent {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing?: boolean;
  keyCode?: number;
  defaultPrevented?: boolean;
  target: unknown;
}

function isTargetLike(t: unknown): t is KeyTargetLike {
  return typeof t === "object" && t !== null && typeof (t as KeyTargetLike).closest === "function";
}

/**
 * `armed`: the card has been on screen long enough that an Enter typed before it appeared cannot
 * allow it. `canAllow`: this card may be allowed without reading it (never a payload to review,
 * an outgoing message or a mail).
 */
export function approvalKeyAction(
  e: ApprovalKeyEvent,
  state: { armed: boolean; canAllow: boolean }
): "allow" | "deny" | null {
  if (e.key !== "Enter" && e.key !== "Escape") return null;
  if (e.defaultPrevented || e.isComposing || e.keyCode === 229) return null;
  if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return null;
  const target = e.target;
  if (isTargetLike(target)) {
    if (target.closest(DIALOG)) return null;
    const onComposer = target.closest("[data-composer-input]") !== null;
    const inDock = target.closest("[data-approval-dock]") !== null;
    const onPage = target.tagName === "BODY" || target.tagName === "HTML";
    if (onComposer) {
      if (typeof target.value === "string" && target.value.length > 0) return null;
    } else if (inDock) {
      // A field or a button of the dock keeps its own Enter / Escape.
      if (target.closest(EDITABLE)) return null;
      if (e.key === "Enter" && target.closest(INTERACTIVE)) return null;
    } else if (!onPage) {
      return null;
    }
  }
  if (e.key === "Escape") return "deny";
  return state.armed && state.canAllow ? "allow" : null;
}
