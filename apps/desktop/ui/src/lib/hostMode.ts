/**
 * What the app does while a paired always-on host is the selected computer.
 *
 * Pure helpers: the local first-run wizard never runs against a host, the host's refusals
 * (`crates/daemon-api/src/peer.rs`) are shown as one human sentence by `humanError`, and the entry point for
 * "connect a model on this server" sits behind a single flag.
 */

/** Same id as `LOCAL_COMPUTER` in the daemon context (kept literal so this file stays pure). */
const LOCAL = "local";

/**
 * Sending a provider key from this computer to a host is NOT built yet (owner approval pending). While this is `false` the "Connect a model on this server" row only explains the
 * command to run on the server. The real flow is enabled by flipping this flag and rendering the
 * key form where `HostEmptyState` renders `HostModelDialog`.
 */
export const HOST_MODEL_KEY_FROM_APP = false;

/** What to run in a terminal on the server to connect a model there. */
export const HOST_MODEL_SETUP_COMMAND = "bot setup";

/** The local first-run wizard only ever talks to this computer's own daemon. */
export function onboardingAllowed(activeComputerId: string): boolean {
  return activeComputerId === LOCAL;
}

/**
 * Refusals an older host (or a computer that is not yet confirmed as administrator) gives for things
 * the owner's confirmed desktop CAN do on a current host. One sentence tells the person what to do
 * (confirm this computer, update the host) instead of a dead end. Matched on the lower-cased English
 * text the daemon sends; those strings are never shown to people.
 */
const NEEDS_ADMIN: readonly string[] = [
  "this needs the owner key",
  "the execution policy is managed on the host",
  "secrets are managed on the host",
  "tool servers and skills are managed on the host",
  "mcp server management is",
  "mcp sign-in is a host-local",
  "account credentials are managed on the host",
  "permissions and tools of a bot on a host are managed on the host",
  "knowledge settings are managed by the local owner",
  "knowledge sources and embedding authority",
  "host-local provider management is not allowed",
  "connection tests spend this host",
  "recent logs are available only",
  "the audit log is available only",
  "the outgoing-mail guard",
  "bot profile files are handled by the owner",
  "site icons are fetched for the local owner",
  "data folder sizes and clear cache",
  "embedding providers and their keys",
  "trusted folders are a host-local",
  "incoming webhooks are managed on this computer",
  "mailboxes are read from the owner's desktop",
  "per-site and per-app permission rules are managed",
  "personal memory is managed by the local owner",
  "can only be changed at this computer",
  "fill secrets are managed by the local owner",
  "shared skill management is local-owner only"
];

/**
 * Host refusal messages (daemon `403` bodies) mapped to i18n keys under `hostMode.refusal`.
 * Matched on the English text the daemon sends; the daemon strings are not shown to people.
 * Order matters: the first needle found wins.
 */
const REFUSALS: ReadonlyArray<readonly [needle: string, key: string]> = [
  ["the owner must confirm this desktop on the host", "adminRequired"],
  ...NEEDS_ADMIN.map((needle) => [needle, "needsAdmin"] as const),
  // What a host never lets the app do, even as administrator (see `peer.rs`, `ADMIN_DESKTOP_NEVER`).
  ["host transfer and channel pairing are done on the host", "hostTransfer"],
  ["pairing and host lifecycle are managed on the host", "lifecycle"],
  ["host links are managed by the local user only", "hostOnly"],
  ["the mcp server facade is host-local", "hostOnly"],
  ["host-local diagnostics are not allowed", "hostOnly"],
  ["not a peer path", "hostOnly"],
  ["screen permissions of devices are granted on the host", "hostOnly"],
  ["composio connections are managed by the owner", "ownerOnly"],
  ["only the local user answers overlay questions", "overlay"],
  ["only the local user may do this", "localOnly"],
  ["only the local user can clear it", "localOnly"],
  ["the local user stopped the takeover", "localOnly"],
  ["screen:input denied", "screenInput"],
  ["screen:view denied", "screenInput"],
  ["input grant for this device", "screenInput"],
  ["local-owner only", "ownerOnly"],
  ["peer role does not allow this operation", "role"],
  ["paired peer identity required", "role"]
];

/**
 * i18n key (under `hostMode.refusal.`) for a refusal, or `null` when it is not one. The stable
 * error `code` counts as much as the text: a daemon that answers `owner_key_required` is asking
 * for what the host's administrator desktop has.
 */
export function hostRefusalKey(status: number | undefined, message: string, code?: string): string | null {
  if (code === "owner_key_required") return "needsAdmin";
  if (code === "host_admin_required") return "adminRequired";
  const text = message.toLowerCase();
  for (const [needle, key] of REFUSALS) {
    if (text.includes(needle)) {
      return key;
    }
  }
  return status === 403 ? "generic" : null;
}

/** What a paired desktop may do on a host today (`peer.rs`), for the empty-state note. */
export const HOST_DESKTOP_MAY = ["bots", "chats", "screen"] as const;
