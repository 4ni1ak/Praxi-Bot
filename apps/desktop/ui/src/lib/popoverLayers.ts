import type { ModalStack } from "./modalStack.ts";

/** What a Popover does with Escape: only the topmost layer reacts (a dialog above keeps the key). */
export function popoverEscapeAction(stack: ModalStack, seq: number, key: string): "close" | null {
  return key === "Escape" && stack.isTop(seq) ? "close" : null;
}

/** What a Popover does with a press: outside presses close it only while it is the topmost layer. */
export function popoverPressAction(stack: ModalStack, seq: number, insidePopoverOrAnchor: boolean): "close" | null {
  return stack.isTop(seq) && !insidePopoverOrAnchor ? "close" : null;
}
