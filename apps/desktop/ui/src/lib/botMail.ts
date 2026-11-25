import type { Bot, EmailAccount, EmailProbeStatus, EmailTestReport } from "../api/types.ts";
import type { PatchOp } from "./botPatch.ts";
import { usableBy } from "./mailIdentity.ts";

/** The glob a profile uses to hand a bot the whole `email_*` tool family. */
export const MAIL_TOOL_GLOB = "email_*";

/** Dispatched when a mailbox is added, changed or removed, so open panels reload. */
export const MAIL_CHANGED_EVENT = "praxi:mail-changed";

/** The mailboxes a bot may use: its own, those shared with it, and the ones any bot may use. */
export function accountsForBot(accounts: readonly EmailAccount[], botId: string): EmailAccount[] {
  return accounts.filter((a) => usableBy(a, botId));
}

function list(config: unknown, key: "allow" | "exclude"): string[] {
  const tools = (config as { tools?: Record<string, unknown> } | null | undefined)?.tools;
  const v = tools?.[key];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

/** Does a profile glob (`*` only) match a tool name? */
export function globMatches(glob: string, name: string): boolean {
  const re = new RegExp(
    `^${glob
      .split("*")
      .map((p) => p.replace(/[.+?^${}()|[\]\\]/g, "\\$&"))
      .join(".*")}$`
  );
  return re.test(name);
}

const matchesAny = (globs: readonly string[], name: string) => globs.some((g) => globMatches(g, name));

const PROBE = "email_read";

function visible(allow: readonly string[], exclude: readonly string[], name: string): boolean {
  return (allow.length === 0 || matchesAny(allow, name)) && !matchesAny(exclude, name);
}

/**
 * Whether the bot is offered the mail tools: an empty allow list shows every tool, otherwise a
 * mail glob has to be in it, and no exclude entry may hide them.
 */
export function mailToolsOn(config: unknown): boolean {
  return visible(list(config, "allow"), list(config, "exclude"), PROBE);
}

const isMailEntry = (g: string) => g.startsWith("email_") || g === MAIL_TOOL_GLOB;

/**
 * Profile edits that turn the mail tools on or off, touching nothing but the mail entries of
 * `[tools] allow` / `exclude`.  Empty when the bot is already in the wanted state.
 */
export function mailToolsOps(config: unknown, on: boolean): PatchOp[] {
  if (mailToolsOn(config) === on) return [];
  const allow = list(config, "allow");
  const exclude = list(config, "exclude");
  const ops: PatchOp[] = [];
  if (on) {
    const rest = exclude.filter((g) => !isMailEntry(g));
    if (rest.length !== exclude.length) {
      ops.push(rest.length > 0 ? { op: "set", path: "/tools/exclude", value: rest } : { op: "unset", path: "/tools/exclude" });
    }
    if (allow.length > 0 && !matchesAny(allow, PROBE)) {
      ops.push({ op: "set", path: "/tools/allow", value: [...allow, MAIL_TOOL_GLOB] });
    }
    return ops;
  }
  const rest = allow.filter((g) => !isMailEntry(g));
  if (allow.length > 0 && rest.length > 0) {
    // "Only these tools" stays a filter; the mail entries just leave it.
    if (rest.length !== allow.length) ops.push({ op: "set", path: "/tools/allow", value: rest });
  } else {
    // An empty allow list means "every tool": hide the mail family by exclusion instead.
    ops.push({ op: "set", path: "/tools/exclude", value: [...exclude, MAIL_TOOL_GLOB] });
  }
  return ops;
}

/** Version-checked patch body for `PATCH /v1/bots/{id}`. */
export function mailToolsPatch(bot: Bot, on: boolean): { expected_version: number; ops: PatchOp[] } {
  return { expected_version: bot.version, ops: mailToolsOps(bot.config, on) };
}

export type MailNextStep =
  | "ok"
  | "authFailed"
  | "apiKey"
  | "unreachable"
  | "tls"
  | "protocol"
  | "timeout";

/** The one status that matters: the first server that did not pass. */
export function worstStatus(report: EmailTestReport): EmailProbeStatus {
  const order: EmailProbeStatus[] = report.api ? [report.api] : [report.imap, report.smtp];
  return order.find((s) => s !== "ok") ?? "ok";
}

/** Which "what to do next" sentence fits a test report (an i18n key suffix). */
export function nextStep(report: EmailTestReport): MailNextStep {
  const s = worstStatus(report);
  if (s === "ok") return "ok";
  if (s === "auth_failed") return report.api ? "apiKey" : "authFailed";
  if (s === "unreachable") return "unreachable";
  if (s === "tls") return "tls";
  if (s === "timeout") return "timeout";
  return "protocol";
}
