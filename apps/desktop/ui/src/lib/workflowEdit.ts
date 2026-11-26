/** Pure helpers for the workflow editor: readable steps, edit rows, import checks. */

export const MAX_IMPORT_BYTES = 256 * 1024;

export interface StepView {
  kind: string;
  /** Raw action word (click, type, ...), empty when the kind says it all. */
  action: string;
  /** What the step acts on: element, address, program, command... */
  target: string;
  /** Text typed or set, with `{name}` placeholders left as they are. */
  value: string | null;
  /** A secret parameter is typed here; its value is never stored or shown. */
  secretName: string | null;
}

type Raw = Record<string, unknown>;
const str = (v: unknown): string => (typeof v === "string" ? v : "");

function locator(step: Raw): string {
  const t = step.target as Raw | null | undefined;
  if (!t) return "";
  return [str(t.role), str(t.name)].filter(Boolean).join(" ");
}

export function stepView(step: unknown): StepView {
  const s = (step ?? {}) as Raw;
  const kind = str(s.kind) || "unknown";
  const base: StepView = { kind, action: "", target: "", value: null, secretName: null };
  switch (kind) {
    case "desktop_action":
    case "browser_action":
      return {
        ...base,
        action: str(s.action),
        target: locator(s) || str(s.app) || str(s.url_pattern),
        value: typeof s.value === "string" ? s.value : null,
        secretName: typeof s.secret === "string" ? s.secret : null
      };
    case "navigate":
      return { ...base, target: str(s.url) };
    case "launch":
      return { ...base, target: str(s.executable) };
    case "wait_for":
      return { ...base, target: str(s.window_title_pattern) || str(s.text) || str(s.selector) };
    case "shell":
      return { ...base, target: str(s.command) };
    case "tool":
      return { ...base, target: str(s.tool) };
    case "agent":
      return { ...base, target: str(s.instruction) };
    default:
      return base;
  }
}

/** `{name}` placeholders used in a step, in order of appearance, without duplicates. */
export function stepParams(step: unknown): string[] {
  const found = new Set<string>();
  for (const m of JSON.stringify(step ?? {}).matchAll(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g)) found.add(m[1]);
  return [...found];
}

export interface WaitDraft {
  /** Which condition this wait uses. */
  mode: "text" | "window" | "idle";
  value: string;
  timeoutSecs: number;
}

export type EditRow = { keep: number } | { wait: WaitDraft };

export const rowsOf = (count: number): EditRow[] => Array.from({ length: count }, (_, keep) => ({ keep }));

export function moveRow(rows: EditRow[], index: number, delta: -1 | 1): EditRow[] {
  const to = index + delta;
  if (index < 0 || index >= rows.length || to < 0 || to >= rows.length) return rows;
  const next = rows.slice();
  [next[index], next[to]] = [next[to], next[index]];
  return next;
}

export const removeRow = (rows: EditRow[], index: number): EditRow[] => rows.filter((_, i) => i !== index);

export function waitProblem(w: WaitDraft): "empty" | "timeout" | null {
  if (w.mode !== "idle" && w.value.trim() === "") return "empty";
  if (!Number.isInteger(w.timeoutSecs) || w.timeoutSecs < 1 || w.timeoutSecs > 600) return "timeout";
  return null;
}

/** The request steps for the daemon: kept steps by index, new waits spelled out. */
export function editSteps(rows: EditRow[]) {
  return rows.map((row) => {
    if ("keep" in row) return { keep: row.keep };
    const { mode, value, timeoutSecs } = row.wait;
    return {
      wait: {
        text: mode === "text" ? value.trim() : null,
        window_title: mode === "window" ? value.trim() : null,
        idle: mode === "idle",
        timeout_secs: timeoutSecs
      }
    };
  });
}

export type CheckKind = "window_title" | "browser_text" | "desktop_text";
export const CHECK_KINDS: CheckKind[] = ["browser_text", "desktop_text", "window_title"];

/** The saved final check as an editable pair, or null when there is none. */
export function checkOf(raw: unknown): { kind: CheckKind; text: string } | null {
  const c = raw as Raw | null | undefined;
  if (!c) return null;
  const kind = str(c.kind) as CheckKind;
  if (!CHECK_KINDS.includes(kind)) return null;
  return { kind, text: kind === "window_title" ? str(c.pattern) : str(c.contains) };
}

export type ImportProblem = "tooBig" | "notJson" | "notWorkflow";

/** Cheap client-side pre-check; the daemon repeats the strict validation. */
export function importProblem(text: string): ImportProblem | null {
  if (new TextEncoder().encode(text).length > MAX_IMPORT_BYTES) return "tooBig";
  try {
    const v: unknown = JSON.parse(text);
    return v && typeof v === "object" && !Array.isArray(v) && Array.isArray((v as Raw).steps) ? null : "notWorkflow";
  } catch {
    return "notJson";
  }
}

/** Suggested copy name that is free: `name-copy`, `name-copy-2`, ... within 64 characters. */
export function copyName(name: string, taken: string[]): string {
  const base = name.slice(0, 58);
  let candidate = `${base}-copy`;
  for (let n = 2; taken.includes(candidate); n += 1) candidate = `${base}-copy-${n}`;
  return candidate;
}
