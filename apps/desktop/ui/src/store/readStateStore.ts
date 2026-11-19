import { useSyncExternalStore } from "react";
import { emptyReadState, parseReadState, withRead, type ReadState } from "../lib/unread";

/** Per-server read positions, mirrored to localStorage (this profile only; see lib/unread). */
const KEY = (host: string) => `praxi.readState.${host}`;
const cache = new Map<string, ReadState>();
const listeners = new Set<() => void>();

function load(host: string): ReadState {
  const hit = cache.get(host);
  if (hit) return hit;
  let state: ReadState;
  try {
    const raw = window.localStorage.getItem(KEY(host));
    state = parseReadState(raw ? JSON.parse(raw) : null, Date.now());
    if (!raw) window.localStorage.setItem(KEY(host), JSON.stringify(state));
  } catch {
    state = emptyReadState(Date.now());
  }
  cache.set(host, state);
  return state;
}

export function markChatRead(host: string, chatId: string, ms: number): void {
  const prev = load(host);
  const next = withRead(prev, chatId, ms);
  if (next === prev) return;
  cache.set(host, next);
  try {
    window.localStorage.setItem(KEY(host), JSON.stringify(next));
  } catch {
    // blocked storage: the position lives for this session only
  }
  listeners.forEach((l) => l());
}

export function useReadState(host: string): ReadState {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => load(host)
  );
}
