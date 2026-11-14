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

export type BotAction = "create" | "update" | "delete";

export interface BotFieldChange {
  /** Dotted path in the profile, e.g. `permissions.preset`. */
  field: string;
  from: string | null;
  to: string | null;
}

/** What the `bot_*` tools ask approval for: the complete resulting profile. */
export interface BotApprovalDetails {
  action: BotAction;
  name: string;
  persona: string;
  /** `provider/model` per role. */
  executor: string;
  planner: string;
  classifier: string;
  /** Preset name, or `custom` when the policy lists categories. */
  preset: string;
  toolsAllow: string[];
  toolsExclude: string[];
  color: string;
  /** Avatar as an image data URL (`data:image/...`), when set. */
  avatar: string | null;
  /** For an update: every field that changes. */
  changes: BotFieldChange[];
  /** The whole profile as JSON text (avatar bytes elided). */
  profileJson: string;
}

const text = (v: unknown): string => (typeof v === "string" ? v : "");

/** Parse `details` of an approval; `null` when it is not a bot card. */
export function botDetails(details: unknown): BotApprovalDetails | null {
  if (!isObject(details) || details.kind !== "bot") return null;
  const action = details.action;
  if (action !== "create" && action !== "update" && action !== "delete") return null;
  const profile = isObject(details.profile) ? details.profile : {};
  const bot = isObject(profile.bot) ? profile.bot : {};
  const models = isObject(profile.models) ? profile.models : {};
  const perms = isObject(profile.permissions) ? profile.permissions : {};
  const tools = isObject(profile.tools) ? profile.tools : {};
  const cats = isObject(perms.categories) ? Object.keys(perms.categories) : [];
  const avatar = typeof details.avatar === "string" && /^data:image\/(png|jpeg|gif|webp);base64,/.test(details.avatar)
    ? details.avatar
    : null;
  const changes = Array.isArray(details.changes)
    ? details.changes.filter(isObject).map((c) => ({
        field: text(c.field),
        from: typeof c.from === "string" ? c.from : null,
        to: typeof c.to === "string" ? c.to : null
      }))
    : [];
  return {
    action,
    name: text(details.name),
    persona: text(bot.persona),
    executor: text(models.executor),
    planner: text(models.planner),
    classifier: text(models.classifier),
    preset: cats.length > 0 ? "custom" : text(perms.preset).replace(/_/g, "-") || "inherited",
    toolsAllow: strings(tools.allow),
    toolsExclude: strings(tools.exclude),
    color: /^#[0-9a-fA-F]{6}$/.test(text(bot.color)) ? text(bot.color) : "",
    avatar,
    changes,
    profileJson: JSON.stringify(profile, null, 2)
  };
}
