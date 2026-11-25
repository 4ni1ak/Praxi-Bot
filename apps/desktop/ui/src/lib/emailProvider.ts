/**
 * What a mail provider (AgentMail) said no to, in plain words. The daemon answers `502
 * email_provider_error` with ONE coarse sentence (never the provider's own text, never a key,
 * see `ProviderError` in `crates/connector-email`); this file turns that sentence into a kind the
 * app words with `errors.emailProvider.<kind>`. A 502 from the provider is NOT "the host does not
 * answer": the host answered, the provider refused.
 */
export type EmailProviderKind =
  | "auth"
  | "forbidden"
  | "quota"
  | "rateLimited"
  | "transient"
  | "taken"
  | "rejected"
  | "other";

/** Provider error codes that mean "this name is already used". */
const TAKEN_CODE = /already|taken|exist|conflict|duplicate|in_use/;

export function emailProviderKind(message: string): EmailProviderKind {
  const text = message.toLowerCase();
  if (text.includes("rejected the api key")) return "auth";
  if (text.includes("not allowed to do this")) return "forbidden";
  if (text.includes("limit of the provider plan")) return "quota";
  if (text.includes("rate limited")) return "rateLimited";
  if (text.includes("temporarily unavailable")) return "transient";
  if (text.includes("rejected the request")) {
    const code = /\(([a-z0-9_]{1,64})\)/.exec(text)?.[1] ?? "";
    return TAKEN_CODE.test(code) ? "taken" : "rejected";
  }
  return "other";
}

/**
 * A free-looking alternative for an inbox name the provider says is taken: the wished name (or
 * `praxi`) plus a short random suffix. `random` is injected so the result is testable.
 */
export function suggestInboxName(wished: string, random: () => number = Math.random): string {
  const base =
    wished
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 24) || "praxi";
  const suffix = Math.floor(random() * 36 ** 4)
    .toString(36)
    .padStart(4, "0");
  return `${base}-${suffix}`;
}

/** Whether `e` is the daemon's `email_provider_error` saying the inbox name is already taken. */
export function isTakenInboxError(e: unknown): boolean {
  const env = (e && typeof e === "object" ? e : {}) as { code?: unknown; message?: unknown };
  return env.code === "email_provider_error" && typeof env.message === "string" && emailProviderKind(env.message) === "taken";
}
