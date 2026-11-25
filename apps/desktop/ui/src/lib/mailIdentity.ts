import type { EmailAccount } from "../api/types.ts";

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** Can this bot use the mailbox: unbound, its own, or shared with it. */
export function usableBy(account: EmailAccount, botId: string): boolean {
  if (account.bot === null || account.bot === undefined) return true;
  return same(account.bot, botId) || (account.shared_with ?? []).some((b) => same(b, botId));
}

/** The identities a bot may send from, default first, then by name. */
export function identitiesFor(accounts: readonly EmailAccount[], botId: string): EmailAccount[] {
  return accounts
    .filter((a) => usableBy(a, botId))
    .sort((a, b) => Number(isDefaultFor(b, botId)) - Number(isDefaultFor(a, botId)) || a.name.localeCompare(b.name));
}

export function isDefaultFor(account: EmailAccount, botId: string): boolean {
  return (account.default_for ?? []).some((b) => same(b, botId));
}

/**
 * The identity a mail call will use: the one it names, else the bot's default, else its own or
 * the only one.  `null` when it cannot be told (the card then just shows the arguments).
 */
export function senderOf(
  accounts: readonly EmailAccount[],
  botId: string | null | undefined,
  input: unknown
): EmailAccount | null {
  const named = (input as { account?: unknown } | null | undefined)?.account;
  if (typeof named === "string") return accounts.find((a) => a.name === named) ?? null;
  if (!botId) return null;
  const mine = identitiesFor(accounts, botId);
  return mine.find((a) => isDefaultFor(a, botId)) ?? (mine.length === 1 ? mine[0] : null);
}

/**
 * The sender the DAEMON resolved for this call: the `from: address (account: name)` line of the
 * approval details. It is the truth (it applies the bot's bindings and defaults on the server);
 * `senderOf` is only a guess for old events that carry no such line.
 */
export function daemonSender(payload: string | null | undefined): { address: string; name: string } | null {
  const m = /^from: (.+?) \(account: ([^)]+)\)/m.exec(payload ?? "");
  return m ? { address: m[1], name: m[2] } : null;
}

/** AgentMail's shared domain: fine for a bot's own sign-ups, not for cold mail. */
export function isSharedDomain(account: Pick<EmailAccount, "provider" | "address">): boolean {
  return account.provider === "agentmail" && /@agentmail\.to$/i.test(account.address);
}

/**
 * What the model is told when the owner picks another sender on the card: the call is denied
 * and the bot asks again with this identity, so the owner reads the new card in full.
 */
export function switchReason(account: Pick<EmailAccount, "name" | "address">): string {
  return `The owner chose to send from another address. Send this same message again with account "${account.name}" (${account.address}) and say that you used it.`;
}
