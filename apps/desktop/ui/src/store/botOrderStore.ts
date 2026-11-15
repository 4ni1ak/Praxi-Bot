import { useSyncExternalStore } from "react";
import { parseBotOrder } from "../lib/botOrder";
import { loadSettingsSync } from "./settingsStore";

/**
 * Live copy of `AppSettings.botOrder` for providers that sit above the settings state
 * (DaemonProvider sorts `bots` with it).  AppShell pushes every settings change in here.
 */
let order: readonly string[] = parseBotOrder(loadSettingsSync()?.botOrder);
const listeners = new Set<() => void>();

export function setBotOrderSnapshot(next: readonly string[]): void {
  if (next.length === order.length && next.every((id, i) => id === order[i])) return;
  order = next;
  listeners.forEach((l) => l());
}

export function useBotOrder(): readonly string[] {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => order
  );
}
