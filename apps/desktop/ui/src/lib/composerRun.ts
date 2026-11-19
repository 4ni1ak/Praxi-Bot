/**
 * The composer's round button (design v7): an arrow to send, a rounded-square STOP while the bot
 * works and the box is empty. With text in the box it is the arrow again (the message is queued).
 */
export type SendButtonMode = "send" | "stop";

export function sendButtonMode(input: { runActive: boolean; hasText: boolean }): SendButtonMode {
  return input.runActive && !input.hasText ? "stop" : "send";
}

/** The button cannot be pressed: nothing to send, a send in flight, or a stop already under way. */
export function sendButtonDisabled(input: {
  mode: SendButtonMode;
  sending: boolean;
  stopping: boolean;
}): boolean {
  return input.mode === "stop" ? input.stopping : input.sending;
}
