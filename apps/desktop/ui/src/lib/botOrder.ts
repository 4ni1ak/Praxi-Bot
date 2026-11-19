/**
 * Sidebar bot order (254-F).  The owner's order is a list of bot ids kept in the app settings;
 * the daemon's list order is only the fallback.  The first bot is the MAIN bot: everything that
 * needs a default bot (`bots[0]`) gets it for free because `useDaemon().bots` is already ordered.
 */

/** `bots` sorted by `order`; bots missing from `order` (new ones) follow in their original order. */
export function orderBots<T extends { id: string }>(bots: readonly T[], order: readonly string[]): T[] {
  const rank = new Map(order.map((id, i) => [id, i]));
  return bots
    .map((bot, i) => ({ bot, i }))
    .sort((a, b) => (rank.get(a.bot.id) ?? Infinity) - (rank.get(b.bot.id) ?? Infinity) || a.i - b.i)
    .map((x) => x.bot);
}

/** Move `id` to index `to` (clamped); returns the same array when nothing changes. */
export function moveBotTo(ids: readonly string[], id: string, to: number): string[] {
  const from = ids.indexOf(id);
  if (from < 0) return [...ids];
  const target = Math.max(0, Math.min(ids.length - 1, to));
  if (target === from) return [...ids];
  const next = ids.filter((x) => x !== id);
  next.splice(target, 0, id);
  return next;
}

/** One-click "Make main": `id` first, everyone else keeps their place (the first bot is the main bot). */
export function makeBotMain(ids: readonly string[], id: string): string[] {
  return moveBotTo(ids, id, 0);
}

/** Keyboard step: one place up (-1) or down (+1). */
export function moveBotBy(ids: readonly string[], id: string, delta: -1 | 1): string[] {
  return moveBotTo(ids, id, ids.indexOf(id) + delta);
}

/** Drop stale ids and keep a stable list for storage. */
export function parseBotOrder(value: unknown): string[] {
  return Array.isArray(value) ? [...new Set(value.filter((x): x is string => typeof x === "string"))] : [];
}
