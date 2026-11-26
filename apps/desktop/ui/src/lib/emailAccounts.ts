import type {
  Bot,
  EmailAccount,
  EmailAccountInput,
  EmailAccountPatch,
  EmailProbeStatus,
  EmailSecurity,
  EmailSendMode,
  EmailTestReport
} from "../api/types.ts";
import type { BotScope } from "./mcpApps.ts";

/** The slice of the daemon client the e-mail card needs (a mock fits too). */
export type EmailApi = {
  listEmailAccounts: () => Promise<EmailAccount[]>;
  createEmailAccount: (input: EmailAccountInput) => Promise<EmailAccount>;
  updateEmailAccount: (
    name: string,
    patch: EmailAccountPatch
  ) => Promise<EmailAccount>;
  deleteEmailAccount: (
    name: string,
    opts?: { deleteInbox?: boolean }
  ) => Promise<void>;
  testEmailAccount: (name: string) => Promise<EmailTestReport>;
  testEmailDraft: (input: EmailAccountInput) => Promise<EmailTestReport>;
};

/** Where the AgentMail pricing (and so the free-plan limits) is published. */
export const AGENTMAIL_PRICING_URL = "https://agentmail.to/pricing";
/** Where a user creates their own AgentMail account and API key. */
export const AGENTMAIL_CONSOLE_URL = "https://console.agentmail.to";

/** The IMAP/SMTP request body: the server fields are always present. */
export type ImapSmtpInput = EmailAccountInput &
  Required<Pick<EmailAccountInput, "address" | "imap" | "smtp">>;

export type EmailPresetId = "gmail" | "outlook" | "yahoo" | "icloud" | "custom";

export interface EmailPreset {
  id: Exclude<EmailPresetId, "custom">;
  imapHost: string;
  imapPort: number;
  smtpHost: string;
  smtpPort: number;
  smtpSecurity: EmailSecurity;
}

/** Well-known providers. IMAP is always implicit TLS on 993. */
export const EMAIL_PRESETS: readonly EmailPreset[] = [
  {
    id: "gmail",
    imapHost: "imap.gmail.com",
    imapPort: 993,
    smtpHost: "smtp.gmail.com",
    smtpPort: 465,
    smtpSecurity: "implicit"
  },
  {
    id: "outlook",
    imapHost: "outlook.office365.com",
    imapPort: 993,
    smtpHost: "smtp-mail.outlook.com",
    smtpPort: 587,
    smtpSecurity: "start_tls"
  },
  {
    id: "yahoo",
    imapHost: "imap.mail.yahoo.com",
    imapPort: 993,
    smtpHost: "smtp.mail.yahoo.com",
    smtpPort: 465,
    smtpSecurity: "implicit"
  },
  {
    id: "icloud",
    imapHost: "imap.mail.me.com",
    imapPort: 993,
    smtpHost: "smtp.mail.me.com",
    smtpPort: 587,
    smtpSecurity: "start_tls"
  }
];

/** What the add / edit form holds. Ports are text while they are typed. */
export interface EmailForm {
  preset: EmailPresetId;
  name: string;
  address: string;
  username: string;
  imapHost: string;
  imapPort: string;
  smtpHost: string;
  smtpPort: string;
  smtpSecurity: EmailSecurity;
  /** Write-only: never filled from a stored account. */
  password: string;
  /**
   * Which bot owns the account: everyone, or exactly one bot.  "Only this bot"
   * with no bot picked yet is an error, never "everyone".
   */
  bot: BotScope;
  /** Bots that use this address as their default sender. */
  defaultFor: string[];
  /** Draft only, or ask the owner every time. */
  sendMode: EmailSendMode;
  /** Allowed senders, one per line or separated by commas. */
  allowedSenders: string;
}

export function emptyForm(): EmailForm {
  return {
    preset: "custom",
    name: "",
    address: "",
    username: "",
    imapHost: "",
    imapPort: "993",
    smtpHost: "",
    smtpPort: "465",
    smtpSecurity: "implicit",
    password: "",
    // Least privilege: a new mailbox belongs to no bot until one is picked.
    bot: { mode: "some", ids: [] },
    defaultFor: [],
    sendMode: "ask",
    allowedSenders: ""
  };
}

/** Prefill the servers; `custom` keeps whatever is typed. */
export function applyPreset(form: EmailForm, id: EmailPresetId): EmailForm {
  const p = EMAIL_PRESETS.find((x) => x.id === id);
  if (!p) {
    return { ...form, preset: "custom" };
  }
  return {
    ...form,
    preset: p.id,
    imapHost: p.imapHost,
    imapPort: String(p.imapPort),
    smtpHost: p.smtpHost,
    smtpPort: String(p.smtpPort),
    smtpSecurity: p.smtpSecurity
  };
}

/** The preset a stored account matches, else `custom`. */
export function presetOf(a: Pick<EmailAccount, "imap" | "smtp">): EmailPresetId {
  const hit = EMAIL_PRESETS.find(
    (p) =>
      p.imapHost === a.imap.host &&
      p.imapPort === (a.imap.port ?? 993) &&
      p.smtpHost === a.smtp.host &&
      p.smtpPort === (a.smtp.port ?? 465) &&
      p.smtpSecurity === (a.smtp.security ?? "implicit")
  );
  return hit?.id ?? "custom";
}

/** "Everyone" or exactly the one bot (never an empty list from a stored account). */
export function scopeFromBot(
  bot: string | null | undefined,
  sharedWith: readonly string[] = []
): BotScope {
  return bot ? { mode: "some", ids: [bot, ...sharedWith] } : { mode: "all" };
}

/** The bots a scope names (empty for "every bot"). */
export function sharedFromScope(scope: BotScope): string[] {
  return scope.mode === "all" ? [] : scope.ids.slice(1);
}

/** Default-sender bots that are still allowed to use the address. */
export function defaultForFromForm(form: Pick<EmailForm, "bot" | "defaultFor">): string[] {
  return form.bot.mode === "all"
    ? form.defaultFor
    : form.defaultFor.filter((id) => form.bot.mode === "some" && form.bot.ids.includes(id));
}

/**
 * The `bot` value to send: `null` = every bot, an id, or `undefined` when
 * "only this bot" has no bot yet (the form must not be submitted then).
 */
export function botFromScope(scope: BotScope): string | null | undefined {
  if (scope.mode === "all") {
    return null;
  }
  return scope.ids[0];
}

export function formFromAccount(a: EmailAccount): EmailForm {
  return {
    preset: presetOf(a),
    name: a.name,
    address: a.address,
    username: a.username,
    imapHost: a.imap.host,
    imapPort: String(a.imap.port ?? 993),
    smtpHost: a.smtp.host,
    smtpPort: String(a.smtp.port ?? 465),
    smtpSecurity: a.smtp.security ?? "implicit",
    password: "",
    bot: scopeFromBot(a.bot, a.shared_with ?? []),
    defaultFor: a.default_for ?? [],
    sendMode: a.send_mode ?? "ask",
    allowedSenders: a.allowed_senders.join("\n")
  };
}

export type EmailFormError =
  | "name"
  | "address"
  | "imapHost"
  | "imapPort"
  | "smtpHost"
  | "smtpPort"
  | "password"
  | "bot"
  | "senders";

const NAME_RE = /^[A-Za-z0-9_-][A-Za-z0-9._-]{0,63}$/;
const ADDRESS_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const HOST_RE = /^[A-Za-z0-9_](?:[A-Za-z0-9._-]*[A-Za-z0-9_])?$/;
const SENDER_DOMAIN_RE = /^@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+$/;

function validPort(text: string): boolean {
  if (!/^\d{1,5}$/.test(text)) {
    return false;
  }
  const n = Number(text);
  return n >= 1 && n <= 65535;
}

function validHost(host: string): boolean {
  // A single IPv6 literal may hold colons; anything else is plain host syntax.
  return (
    host.length > 0 &&
    host.length <= 253 &&
    !host.includes("..") &&
    (HOST_RE.test(host) || /^[0-9A-Fa-f:]+$/.test(host) && host.includes(":"))
  );
}

/** Senders typed one per line or comma separated. */
export function parseSenders(text: string): string[] {
  const out: string[] = [];
  for (const raw of text.split(/[\s,;]+/)) {
    const s = raw.trim().toLowerCase();
    if (s && !out.includes(s)) {
      out.push(s);
    }
  }
  return out;
}

function sendersValid(list: string[]): boolean {
  return (
    list.length <= 100 &&
    list.every((s) => ADDRESS_RE.test(s) || SENDER_DOMAIN_RE.test(s))
  );
}

function loginOf(address: string, username: string): string {
  return username.trim() || address.trim();
}

/**
 * Whether the stored password may no longer be reused: the servers or the login
 * changed, so it would be sent somewhere else (the daemon refuses this too).
 */
export function needsPassword(form: EmailForm, original: EmailAccount): boolean {
  return (
    form.imapHost.trim().toLowerCase() !== original.imap.host.toLowerCase() ||
    Number(form.imapPort) !== (original.imap.port ?? 993) ||
    form.smtpHost.trim().toLowerCase() !== original.smtp.host.toLowerCase() ||
    Number(form.smtpPort) !== (original.smtp.port ?? 465) ||
    loginOf(form.address, form.username) !== loginOf(original.address, original.username)
  );
}

export function validateForm(
  form: EmailForm,
  opts: { existing: string[]; editing: boolean; original?: EmailAccount }
): EmailFormError[] {
  const errors: EmailFormError[] = [];
  const name = form.name.trim();
  if (!NAME_RE.test(name) || (!opts.editing && opts.existing.includes(name))) {
    errors.push("name");
  }
  if (!ADDRESS_RE.test(form.address.trim())) {
    errors.push("address");
  }
  if (!validHost(form.imapHost.trim())) {
    errors.push("imapHost");
  }
  if (!validPort(form.imapPort.trim())) {
    errors.push("imapPort");
  }
  if (!validHost(form.smtpHost.trim())) {
    errors.push("smtpHost");
  }
  if (!validPort(form.smtpPort.trim())) {
    errors.push("smtpPort");
  }
  const mustType =
    !opts.editing ||
    (opts.original !== undefined && needsPassword(form, opts.original));
  if (mustType && form.password === "") {
    errors.push("password");
  }
  if (botFromScope(form.bot) === undefined) {
    errors.push("bot");
  }
  if (!sendersValid(parseSenders(form.allowedSenders))) {
    errors.push("senders");
  }
  return errors;
}

/** Body of `POST /v1/email/accounts` (and of a draft test). */
export function toInput(form: EmailForm): ImapSmtpInput {
  const bot = botFromScope(form.bot);
  const input: ImapSmtpInput = {
    name: form.name.trim(),
    provider: "imap_smtp",
    address: form.address.trim(),
    username: form.username.trim() || null,
    imap: { host: form.imapHost.trim(), port: Number(form.imapPort) },
    smtp: {
      host: form.smtpHost.trim(),
      port: Number(form.smtpPort),
      security: form.smtpSecurity
    },
    bot: bot ?? null,
    shared_with: sharedFromScope(form.bot),
    default_for: defaultForFromForm(form),
    send_mode: form.sendMode,
    allowed_senders: parseSenders(form.allowedSenders)
  };
  if (form.password !== "") {
    input.password = form.password;
  }
  return input;
}

/** Body of `PUT`: only what differs from the stored account. */
export function toPatch(
  form: EmailForm,
  original: EmailAccount
): EmailAccountPatch {
  const patch: EmailAccountPatch = {};
  const input = toInput(form);
  if (input.address !== original.address) {
    patch.address = input.address;
  }
  if ((input.username ?? "") !== original.username) {
    patch.username = input.username ?? "";
  }
  if (
    input.imap.host !== original.imap.host ||
    input.imap.port !== (original.imap.port ?? 993)
  ) {
    patch.imap = input.imap;
  }
  if (
    input.smtp.host !== original.smtp.host ||
    input.smtp.port !== (original.smtp.port ?? 465) ||
    input.smtp.security !== (original.smtp.security ?? "implicit")
  ) {
    patch.smtp = input.smtp;
  }
  if ((input.bot ?? null) !== (original.bot ?? null)) {
    patch.bot = input.bot ?? null;
  }
  if ((input.shared_with ?? []).join(",") !== (original.shared_with ?? []).join(",")) {
    patch.shared_with = input.shared_with ?? [];
  }
  if ((input.default_for ?? []).join(",") !== (original.default_for ?? []).join(",")) {
    patch.default_for = input.default_for ?? [];
  }
  if (input.send_mode !== (original.send_mode ?? "ask")) {
    patch.send_mode = input.send_mode;
  }
  const senders = input.allowed_senders ?? [];
  if (senders.join("\n") !== original.allowed_senders.join("\n")) {
    patch.allowed_senders = senders;
  }
  if (form.password !== "") {
    patch.password = form.password;
  }
  return patch;
}

export function probeTone(status: EmailProbeStatus): "ok" | "bad" {
  return status === "ok" ? "ok" : "bad";
}

/** Display name of the bot an account belongs to (`null` = every bot). */
export function accountBotName(
  a: Pick<EmailAccount, "bot">,
  bots: Pick<Bot, "id" | "name">[],
  deleted: string
): string | null {
  if (!a.bot) {
    return null;
  }
  return bots.find((b) => b.id === a.bot)?.name ?? deleted;
}

// ---- AgentMail ("give this bot an address") ---------------------------------

/** The add flow's first choice. */
export type AddKind = "own" | "agentmail";

/** What the AgentMail form holds. The key is write-only. */
export interface AgentmailForm {
  /** The user's own AgentMail API key. Never filled from a stored account. */
  apiKey: string;
  /** Optional: the part before the @ of the new address. */
  nameHint: string;
  /** Which bot gets the address; "only this bot" needs a bot picked. */
  bot: BotScope;
  /** Make a new address (needs an organization key), or connect one that already exists. */
  mode?: AgentmailMode;
  /** `connect` only: the address (or id) of the inbox that already exists. */
  inbox?: string;
}

/** Create a new address, or connect an existing inbox (what an inbox-scoped key can do). */
export type AgentmailMode = "create" | "connect";

export function emptyAgentmailForm(): AgentmailForm {
  return { apiKey: "", nameHint: "", bot: { mode: "some", ids: [] }, mode: "create", inbox: "" };
}

export type AgentmailFormError = "apiKey" | "nameHint" | "bot" | "inbox";

const HINT_RE = /^[A-Za-z0-9][A-Za-z0-9 ._-]{0,31}$/;
/** An address or an inbox id as the provider names it: no spaces or path characters. */
const INBOX_RE = /^[A-Za-z0-9._+=~:@<>-]{3,320}$/;

/** AgentMail keys that belong to one inbox start like this; they cannot create inboxes. */
export const INBOX_KEY_PREFIX = "am_us_inbox_";

/** Whether `key` looks like an inbox-scoped key (it can only work on the inbox it belongs to). */
export function isInboxScopedKey(key: string): boolean {
  return key.trim().startsWith(INBOX_KEY_PREFIX);
}

/**
 * Whether a failed "create a new address" tells the user that the key may not create inboxes: the
 * provider answered "not allowed" (a scoped key), so the existing-inbox mode is what they need.
 */
export function suggestsConnectExisting(e: unknown): boolean {
  const err = e as { status?: unknown; code?: unknown; message?: unknown } | null;
  return (
    err?.code === "email_provider_error" &&
    typeof err.message === "string" &&
    /not allowed/i.test(err.message)
  );
}

export function validateAgentmailForm(form: AgentmailForm): AgentmailFormError[] {
  const errors: AgentmailFormError[] = [];
  const key = form.apiKey.trim();
  if (key.length < 8 || key.length > 512 || /\s/.test(key)) {
    errors.push("apiKey");
  }
  if (form.mode === "connect") {
    if (!INBOX_RE.test((form.inbox ?? "").trim())) {
      errors.push("inbox");
    }
  } else {
    const hint = form.nameHint.trim();
    if (hint !== "" && !HINT_RE.test(hint)) {
      errors.push("nameHint");
    }
  }
  if (botFromScope(form.bot) === undefined) {
    errors.push("bot");
  }
  return errors;
}

/** A local account name for a new AgentMail inbox that is not taken yet. */
export function agentmailAccountName(hint: string, existing: string[]): string {
  const slug = hint
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
  const base = slug === "" ? "agentmail" : `agentmail-${slug}`;
  let name = base;
  for (let n = 2; existing.includes(name); n += 1) {
    name = `${base}-${n}`;
  }
  return name;
}

/** Body of `POST /v1/email/accounts` for an AgentMail inbox (new, or an existing one). */
export function toAgentmailInput(
  form: AgentmailForm,
  existing: string[]
): EmailAccountInput {
  const connect = form.mode === "connect";
  const inbox = (form.inbox ?? "").trim();
  // The local part of the address names a connected inbox; the hint names a new one.
  const hint = connect ? inbox.split("@")[0] ?? "" : form.nameHint.trim();
  const input: EmailAccountInput = {
    name: agentmailAccountName(hint, existing),
    provider: "agentmail",
    api_key: form.apiKey.trim(),
    bot: botFromScope(form.bot) ?? null
  };
  if (connect) {
    input.inbox = inbox;
  } else if (hint !== "") {
    input.name_hint = hint;
  }
  return input;
}
