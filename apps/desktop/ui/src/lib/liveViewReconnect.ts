import type { SessionInfo } from "../api/types.ts";
import { reconnectDelayMs } from "./liveViewState.ts";

/** A stream must have stayed up this long before the backoff starts over. */
export const MIN_HEALTHY_MS = 10_000;

export interface SessionLink {
  createSession(): Promise<SessionInfo>;
  /** Open and consume the frame stream; resolves when it ends, rejects on error. */
  openStream(sessionId: string, signal: AbortSignal): Promise<void>;
  deleteSession(sessionId: string): Promise<void>;
}

export interface ReconnectHooks {
  /** A new session exists (first connect, or the old one was gone). */
  onSession(session: SessionInfo): void;
  /** The stream ended or failed: stop input, stop renewing the lease. */
  onDisconnected(): void;
  /** Called before every retry (re-read control state). */
  beforeRetry(): void;
}

function isGone(err: unknown): boolean {
  const status =
    err !== null && typeof err === "object"
      ? (err as { status?: unknown }).status
      : undefined;
  return status === 404 || status === 410;
}

/**
 * Keep one capture session streaming. A dropped stream is retried on the SAME
 * session (the daemon keeps the session, and deleting a session that owns a
 * takeover would make its hand-back impossible); only a session the daemon no
 * longer knows is recreated. The backoff grows on every drop and resets only
 * after the stream stayed up for `MIN_HEALTHY_MS`.
 */
export async function runReconnectLoop(
  link: SessionLink,
  hooks: ReconnectHooks,
  opts: {
    signal: AbortSignal;
    sleep: (ms: number) => Promise<void>;
    now: () => number;
    minHealthyMs?: number;
    /** Receives the delay chosen before each retry (diagnostics / tests). */
    onBackoff?: (ms: number) => void;
  }
): Promise<{ sessionId: string | null }> {
  const minHealthy = opts.minHealthyMs ?? MIN_HEALTHY_MS;
  let session: SessionInfo | null = null;
  let attempt = 0;
  while (!opts.signal.aborted) {
    const startedAt = opts.now();
    try {
      if (!session) {
        const created = await link.createSession();
        if (opts.signal.aborted) {
          void link.deleteSession(created.session_id).catch(() => undefined);
          break;
        }
        session = created;
        hooks.onSession(created);
      }
      await link.openStream(session.session_id, opts.signal);
    } catch (err) {
      if (session && isGone(err)) {
        session = null;
      }
    }
    if (opts.signal.aborted) {
      break;
    }
    hooks.onDisconnected();
    attempt = opts.now() - startedAt >= minHealthy ? 1 : attempt + 1;
    const delay = reconnectDelayMs(attempt);
    opts.onBackoff?.(delay);
    await opts.sleep(delay);
    if (opts.signal.aborted) {
      break;
    }
    hooks.beforeRetry();
  }
  return { sessionId: session?.session_id ?? null };
}
