/**
 * Sending a message while the host is not answering.
 *
 * - This computer's own service: the app starts it again (the same way it does at
 *   launch), waits until it answers and sends the message once more.
 * - A paired server: the app cannot power it on. The message stays a draft and the
 *   user gets an actionable message with Retry.
 *
 * A message is only sent a second time when the service was PROVABLY down before
 * (nothing answered a health probe); a slow answer from a live service never
 * causes a duplicate.
 */

export type EnsureHost =
  /** The active computer is a paired server: nothing the app can start. */
  | { kind: "remote" }
  /** The service answers: the first failure was something else (slow, refused...). */
  | { kind: "reachable" }
  /** The service was down and is up again; `client` talks to it. */
  | { kind: "started"; client: unknown };

export type SendRecovery<T> =
  | { kind: "sent"; value: T; started: boolean }
  | { kind: "remote_offline"; error: unknown }
  | { kind: "start_failed"; error: unknown }
  | { kind: "failed"; error: unknown }
  /** The client was disposed (computer switched) or the caller cancelled: no message at all. */
  | { kind: "cancelled" };

function netKindOf(err: unknown): unknown {
  return err !== null && typeof err === "object" ? (err as { netKind?: unknown }).netKind : undefined;
}

/**
 * True only when the connection itself failed (refused / unreachable): nothing received the
 * request. A timeout or an abort never counts, because the service may have got the message.
 */
export function isNetworkFailure(err: unknown): boolean {
  return (
    err !== null &&
    typeof err === "object" &&
    (err as { code?: unknown }).code === "network" &&
    netKindOf(err) === "connect"
  );
}

/** The request was cancelled on our side (client disposed / caller abort). */
export function isCancelled(err: unknown): boolean {
  return netKindOf(err) === "aborted";
}

export async function sendWithHostRecovery<T>(deps: {
  /** Send through `client` (null: the client the view already has). */
  send: (client: unknown | null) => Promise<T>;
  ensure: () => Promise<EnsureHost>;
  /** Called once, right before the service is started (the visible "starting host..."). */
  onStarting?: () => void;
}): Promise<SendRecovery<T>> {
  try {
    return { kind: "sent", value: await deps.send(null), started: false };
  } catch (first) {
    if (isCancelled(first)) {
      return { kind: "cancelled" };
    }
    if (!isNetworkFailure(first)) {
      return { kind: "failed", error: first };
    }
    deps.onStarting?.();
    let ensured: EnsureHost;
    try {
      ensured = await deps.ensure();
    } catch (e) {
      return { kind: "start_failed", error: e };
    }
    if (ensured.kind === "remote") {
      return { kind: "remote_offline", error: first };
    }
    if (ensured.kind === "reachable") {
      return { kind: "failed", error: first };
    }
    try {
      return { kind: "sent", value: await deps.send(ensured.client), started: true };
    } catch (second) {
      return { kind: "failed", error: second };
    }
  }
}
