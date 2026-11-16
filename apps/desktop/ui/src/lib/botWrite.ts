import type { ApiClient } from "../api/client.ts";
import type { Bot, BotInput } from "../api/types.ts";
import { conflictBot } from "./botPatch.ts";
import { humanLine } from "./humanError.ts";

/** A stale (`409 version_conflict`) or missing (`428`) profile version: reload and try again. */
export function isVersionError(e: unknown): boolean {
  const err = e as { status?: number; code?: string } | null;
  return !!err && ((err.status === 409 && err.code === "version_conflict") || err.status === 428);
}

/**
 * Replace a bot's profile with the version it was loaded at. A version error reloads the bot
 * once and builds the input again from the fresh one; a second one is a real conflict and is
 * thrown (show it with `botWriteMessage`).
 */
export async function writeBotProfile(
  client: Pick<ApiClient, "getBot" | "updateBot">,
  bot: Bot,
  build: (current: Bot) => BotInput
): Promise<Bot> {
  let current = bot;
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await client.updateBot(current.id, {
        ...build(current),
        expected_version: current.version
      });
    } catch (e) {
      if (attempt >= 1 || !isVersionError(e)) throw e;
      current = conflictBot(e) ?? (await client.getBot(current.id));
    }
  }
}

/** Human sentence for a failed bot write: never the daemon's raw version message. */
export function botWriteMessage(e: unknown, t: (key: string) => string): string {
  if (isVersionError(e)) return t("bots.writeConflict");
  return humanLine(e);
}
