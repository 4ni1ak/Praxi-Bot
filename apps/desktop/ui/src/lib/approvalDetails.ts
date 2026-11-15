/** Structured facts a tool attaches to its approval card (`approval_needed.details`). */

export interface SchedulePermission {
  /** `full`, `ask_first`, `mixed` or `read_only`. */
  level: string;
  allow: string[];
  ask: string[];
  deny: string[];
}

export interface PreviousSchedule {
  name: string;
  prompt: string;
  schedule: string;
  timezone: string;
}

export type ScheduleAction = "create" | "update" | "resume" | "pause" | "delete";

/** What the `schedule_*` tools ask approval for. */
export interface ScheduleApprovalDetails {
  action: ScheduleAction;
  name: string;
  /** The exact prompt that will run at every firing (empty for pause / delete). */
  prompt: string;
  /** Prompt length in characters (so a long prompt's size is obvious). */
  promptChars: number;
  /** For an update: what it is now. */
  previous: PreviousSchedule | null;
  /** Human-readable timing. */
  schedule: string;
  timezone: string;
  /** The next firing times (RFC 3339, user's timezone). */
  nextRuns: string[];
  permission: SchedulePermission | null;
}

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const strings = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];

export interface BrainApprovalDetails {
  path: string;
  content: string;
  sha256: string;
  sources: { id: string; revision: number; label: string }[];
}

/** Daemon-frozen content, rendered as text and never as HTML/instructions. */
export function brainDetails(details: unknown): BrainApprovalDetails | null {
  if (!isObject(details) || details.kind !== "knowledge_brain_write" ||
      typeof details.path !== "string" || typeof details.content !== "string" ||
      typeof details.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(details.sha256) ||
      details.content.length > 65_536 || !Array.isArray(details.sources)) return null;
  const sources = details.sources.filter((source): source is { id: string; revision: number; label: string } =>
    isObject(source) && typeof source.id === "string" && typeof source.revision === "number" && typeof source.label === "string");
  if (sources.length !== details.sources.length || sources.length > 16) return null;
  return { path: details.path, content: details.content, sha256: details.sha256, sources };
}

/** Parse `details` of an approval; `null` when it is not a schedule card. */
export function scheduleDetails(details: unknown): ScheduleApprovalDetails | null {
  if (!isObject(details) || details.kind !== "schedule") return null;
  const action = details.action;
  if (
    action !== "create" &&
    action !== "update" &&
    action !== "resume" &&
    action !== "pause" &&
    action !== "delete"
  ) {
    return null;
  }
  const perm = details.permission;
  const prev = details.previous;
  const prompt = typeof details.prompt === "string" ? details.prompt : "";
  return {
    action,
    name: typeof details.name === "string" ? details.name : "",
    prompt,
    promptChars:
      typeof details.prompt_chars === "number" ? details.prompt_chars : [...prompt].length,
    previous: isObject(prev)
      ? {
          name: typeof prev.name === "string" ? prev.name : "",
          prompt: typeof prev.prompt === "string" ? prev.prompt : "",
          schedule: typeof prev.schedule === "string" ? prev.schedule : "",
          timezone: typeof prev.timezone === "string" ? prev.timezone : ""
        }
      : null,
    schedule: typeof details.schedule === "string" ? details.schedule : "",
    timezone: typeof details.timezone === "string" ? details.timezone : "",
    nextRuns: strings(details.next_runs),
    permission: isObject(perm)
      ? {
          level: typeof perm.level === "string" ? perm.level : "",
          allow: strings(perm.allow),
          ask: strings(perm.ask),
          deny: strings(perm.deny)
        }
      : null
  };
}

export interface EgressApprovalDetails {
  reason: string;
  read: string[];
  host: string;
  payload: string;
  payloadKind: string;
  payloadBytes?: number;
  reviewFullPayload: boolean;
  /** Lines in the text that would be sent. Absent on older events. */
  payloadLines?: number;
  /** Characters in the text that would be sent. Absent on older events. */
  payloadChars?: number;
  /** Non-printing characters shown in the payload as `<U+XXXX>`. */
  hiddenChars?: number;
}

/** Parse an egress card, including one nested beside a schedule card. */
export function egressDetails(details: unknown): EgressApprovalDetails | null {
  if (!isObject(details)) return null;
  const raw =
    details.kind === "egress"
      ? details
      : isObject(details.egress) && details.egress.kind === "egress"
        ? details.egress
        : null;
  if (!raw) return null;
  return {
    reason: typeof raw.reason === "string" ? raw.reason : "",
    read: strings(raw.read),
    host: typeof raw.host === "string" ? raw.host : "",
    payload: typeof raw.payload === "string" ? raw.payload : "",
    payloadKind: typeof raw.payload_kind === "string" ? raw.payload_kind : "body",
    payloadBytes: count(raw.payload_bytes),
    reviewFullPayload: raw.review_full_payload === true,
    payloadLines: count(raw.payload_lines),
    payloadChars: count(raw.payload_chars),
    hiddenChars: count(raw.hidden_chars)
  };
}

export interface ComputerApprovalDetails {
  /** `pc`, `host` or `browser`. */
  kind: string;
  name: string;
  /** Immutable node id (a machine fingerprint for a real computer). */
  nodeId: string;
}

// Control, invisible and direction-override characters. A device label is
// chosen by a peer, so it is shown as one short plain line.
const HIDDEN_CLASS = /^[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Co}]$/u;
const HIDDEN_POINTS = new Set([0x34f, 0x115f, 0x1160, 0x17b4, 0x17b5, 0x2800, 0x3164, 0xffa0]);

function isHidden(ch: string): boolean {
  const cp = ch.codePointAt(0) ?? 0;
  return (
    HIDDEN_CLASS.test(ch) ||
    HIDDEN_POINTS.has(cp) ||
    (cp >= 0x180b && cp <= 0x180f) ||
    (cp >= 0xfe00 && cp <= 0xfe0f)
  );
}

function plain(value: unknown, max: number): string {
  if (typeof value !== "string") return "";
  const shown = [...value].filter((ch) => !isHidden(ch)).join("");
  return [...shown.replace(/DAEMON SUMMARY/g, "")].slice(0, max).join("").trim();
}

/**
 * The computer the daemon bound to this call (`details.computer_target`), for
 * a card whose headline is replaced by a schedule or egress body.
 */
export function computerDetails(details: unknown): ComputerApprovalDetails | null {
  if (!isObject(details) || !isObject(details.computer_target)) return null;
  const c = details.computer_target;
  const kind = plain(c.kind, 16);
  const name = plain(c.node_name, 80);
  const nodeId = plain(c.node_id, 80) || "local";
  if (!kind && !name && nodeId === "local") return null;
  return { kind, name, nodeId };
}

function count(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** `2026-11-13T10:30:00+03:00` as `Fri 13 Nov, 10:30` in the offset it carries. */
export function formatFiring(rfc3339: string, locale?: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(rfc3339);
  if (!m) return rfc3339;
  // The wall-clock fields already are the user's local time: format them as UTC
  // so the browser's own timezone cannot shift them.
  const d = new Date(Date.UTC(+m[1], +m[2] - 1, +m[3], +m[4], +m[5]));
  const date = new Intl.DateTimeFormat(locale, {
    weekday: "short",
    day: "numeric",
    month: "short",
    timeZone: "UTC"
  }).format(d);
  return `${date}, ${m[4]}:${m[5]}`;
}

/** Approval becomes available only with the full view open and its end visible. */
export function payloadReviewComplete(
  expanded: boolean,
  view: { scrollTop: number; clientHeight: number; scrollHeight: number }
): boolean {
  return expanded && view.clientHeight > 0 &&
    view.scrollTop + view.clientHeight >= view.scrollHeight - 1;
}

/** App actions carry the complete JSON arguments; malformed cards cannot approve. */
export function appActionDetails(details: unknown) {
  if (!isObject(details) || details.kind !== "app_action" || !isObject(details.arguments) ||
      typeof details.app_name !== "string" || typeof details.action_slug !== "string" ||
      typeof details.version !== "string" || typeof details.class !== "string") return null;
  const compact = JSON.stringify(details.arguments);
  if (new TextEncoder().encode(compact).length > 32 * 1024) return null;
  // Preserve every value while making invisible/bidi characters visible as JSON escapes.
  const argumentsText = JSON.stringify(details.arguments, null, 2).replace(/[\p{Default_Ignorable_Code_Point}\p{Cf}\p{Zl}\p{Zp}\u2800]/gu,
    (c) => c.split("").map((unit) => "\\u" + unit.charCodeAt(0).toString(16).padStart(4, "0")).join(""));
  return { app: details.app_name, action: details.action_slug, version: details.version,
    class: details.class, argumentsText, count: [...argumentsText].length };
}
