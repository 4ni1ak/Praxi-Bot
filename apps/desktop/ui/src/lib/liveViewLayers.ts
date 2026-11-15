import type { ModalStack } from "./modalStack.ts";

export interface ViewerKey {
  key: string;
  shiftKey: boolean;
  defaultPrevented: boolean;
}

/**
 * What the live viewer does with a key. The viewer only owns Tab and Escape while
 * it is the topmost layer (a Modal, Popover or the command palette above it
 * keeps them), and never closes on an Escape another layer already handled.
 */
export function viewerKeyAction(opts: {
  stack: ModalStack;
  seq: number;
  event: ViewerKey;
  interactive: boolean;
  stageFocused: boolean;
}): "trap-tab" | "close" | null {
  const { stack, seq, event } = opts;
  if (!stack.isTop(seq)) {
    return null;
  }
  if (event.key === "Tab") {
    return "trap-tab";
  }
  if (
    event.key === "Escape" &&
    !event.shiftKey &&
    !event.defaultPrevented &&
    !opts.interactive &&
    !opts.stageFocused
  ) {
    return "close";
  }
  return null;
}
