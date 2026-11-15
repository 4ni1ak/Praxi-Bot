import { foldText } from "./searchText.ts";

/**
 * The "Add skill" flow (owner feedback 244, point 4): compose a `SKILL.md` from text or read an
 * existing one, checked against the same rules the daemon's `skills` crate applies
 * (`crates/skills/src/skill.rs`, `frontmatter.rs`), so the file the UI produces is one the
 * daemon will load.
 *
 * The daemon has no route that writes a skill yet (`GET /v1/skills`, `GET|PUT
 * /v1/bots/{id}/skills` only), so the flow ends at a real file plus a re-check of the real
 * catalog - see `SKILL_WRITE_ROUTE`.
 */

export const SKILL_NAME_MAX = 64;
export const SKILL_DESCRIPTION_MAX = 200;
export const SKILL_MAX_BYTES = 64 * 1024;
export const DEFAULT_SKILL_VERSION = "0.1.0";

/** The route the app needs to install a skill itself.  Not implemented by the daemon today. */
export const SKILL_WRITE_ROUTE = "POST /v1/skills";

export interface SkillDraft {
  name: string;
  description: string;
  version: string;
  /** Markdown instructions (the part after the frontmatter). */
  body: string;
  /** Optional `when:` triggers, as the user types them (comma separated). */
  domains: string;
  apps: string;
  keywords: string;
}

export function emptySkillDraft(): SkillDraft {
  return {
    name: "",
    description: "",
    version: DEFAULT_SKILL_VERSION,
    body: "",
    domains: "",
    apps: "",
    keywords: ""
  };
}

export type SkillField = "name" | "description" | "version" | "body" | "file";

export type SkillIssueCode =
  | "nameEmpty"
  | "nameLength"
  | "nameChars"
  | "nameStart"
  | "nameDots"
  | "nameTaken"
  | "descriptionEmpty"
  | "descriptionLong"
  | "descriptionLines"
  | "versionEmpty"
  | "bodyEmpty"
  | "tooLarge";

export interface SkillIssue {
  field: SkillField;
  code: SkillIssueCode;
}

/** `validate_name` of the skills crate: `[a-z0-9._-]`, starts alphanumeric, no `..`, no trailing dot. */
export function skillNameIssue(name: string): SkillIssueCode | null {
  if (name.length === 0) return "nameEmpty";
  if (name.length > SKILL_NAME_MAX) return "nameLength";
  if (!/^[a-z0-9._-]+$/.test(name)) return "nameChars";
  if (!/^[a-z0-9]/.test(name)) return "nameStart";
  if (name.includes("..") || name.endsWith(".")) return "nameDots";
  return null;
}

/** A valid skill name from a free title: "GitHub İnceleme!" -> `github-inceleme`. */
export function suggestSkillName(title: string): string {
  return foldText(title)
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/\.{2,}/g, ".")
    .replace(/-{2,}/g, "-")
    .replace(/^[^a-z0-9]+/, "")
    .slice(0, SKILL_NAME_MAX)
    .replace(/[-_.]+$/, "");
}

/** Split a comma list of triggers; quotes and brackets cannot be written in the frontmatter list. */
export function triggerList(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of text.split(/[,\n]/)) {
    const item = part.replace(/["'[\]]/g, "").trim();
    if (item && !seen.has(item)) {
      seen.add(item);
      out.push(item);
    }
  }
  return out;
}

function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** The scalar the daemon's parser reads back unchanged (it strips one pair of matching quotes). */
function scalar(value: string): string {
  return `"${value.replace(/\r?\n/g, " ").trim()}"`;
}

/** The `SKILL.md` text for a draft. */
export function buildSkillMd(draft: SkillDraft): string {
  const lines = [
    "---",
    `name: ${draft.name.trim()}`,
    `description: ${scalar(draft.description)}`,
    `version: ${scalar(draft.version.trim() || DEFAULT_SKILL_VERSION)}`
  ];
  const when: Array<[string, string[]]> = [
    ["domains", triggerList(draft.domains).map((d) => d.toLowerCase())],
    ["apps", triggerList(draft.apps)],
    ["keywords", triggerList(draft.keywords).map((k) => k.toLowerCase())]
  ];
  const used = when.filter(([, items]) => items.length > 0);
  if (used.length > 0) {
    lines.push("when:");
    for (const [key, items] of used) {
      lines.push(`  ${key}: [${items.map((i) => `"${i}"`).join(", ")}]`);
    }
  }
  lines.push("---", draft.body.trim(), "");
  return lines.join("\n");
}

/** Everything that would make the daemon reject the skill (or shadow an existing one). */
export function validateSkillDraft(
  draft: SkillDraft,
  existingNames: readonly string[] = []
): SkillIssue[] {
  const issues: SkillIssue[] = [];
  const name = draft.name.trim();
  const nameIssue = skillNameIssue(name);
  if (nameIssue) {
    issues.push({ field: "name", code: nameIssue });
  } else if (existingNames.includes(name)) {
    issues.push({ field: "name", code: "nameTaken" });
  }
  const description = draft.description.trim();
  if (!description) {
    issues.push({ field: "description", code: "descriptionEmpty" });
  } else if (/[\r\n]/.test(description)) {
    issues.push({ field: "description", code: "descriptionLines" });
  } else if ([...description].length > SKILL_DESCRIPTION_MAX) {
    issues.push({ field: "description", code: "descriptionLong" });
  }
  if (!draft.version.trim()) issues.push({ field: "version", code: "versionEmpty" });
  if (!draft.body.trim()) {
    issues.push({ field: "body", code: "bodyEmpty" });
  } else if (byteLength(buildSkillMd(draft)) > SKILL_MAX_BYTES) {
    issues.push({ field: "body", code: "tooLarge" });
  }
  return issues;
}

export function issueFor(issues: readonly SkillIssue[], field: SkillField): SkillIssueCode | null {
  return issues.find((i) => i.field === field)?.code ?? null;
}

// --- reading an existing SKILL.md -----------------------------------------

export type SkillParseError =
  | "tooLarge"
  | "noFrontmatter"
  | "unterminated"
  | "badLine"
  | "unknownKey"
  | "badWhen";

export type SkillParseResult =
  | { ok: true; draft: SkillDraft }
  | { ok: false; error: SkillParseError; detail?: string };

function unquote(raw: string): string {
  const s = raw.trim();
  for (const q of ['"', "'"]) {
    if (s.length >= 2 && s.startsWith(q) && s.endsWith(q)) return s.slice(1, -1);
  }
  return s;
}

function listValue(raw: string): string[] | null {
  const s = raw.trim();
  if (!s.startsWith("[") || !s.endsWith("]")) return null;
  return s
    .slice(1, -1)
    .split(",")
    .map((i) => unquote(i))
    .filter(Boolean);
}

/**
 * Read a `SKILL.md` the way the daemon does (the YAML subset of `frontmatter.rs`): the result is
 * an editable draft, so an imported file goes through the same validation as a written one.
 */
export function parseSkillMd(text: string): SkillParseResult {
  if (byteLength(text) > SKILL_MAX_BYTES) return { ok: false, error: "tooLarge" };
  const lines = (text.charCodeAt(0) === 0xfeff ? text.slice(1) : text).split(/\r?\n/);
  if ((lines[0] ?? "").trimEnd() !== "---") return { ok: false, error: "noFrontmatter" };
  const end = lines.findIndex((l, i) => i > 0 && l.trimEnd() === "---");
  if (end < 0) return { ok: false, error: "unterminated" };

  const draft = emptySkillDraft();
  let inWhen = false;
  for (const line of lines.slice(1, end)) {
    if (!line.trim() || line.trimStart().startsWith("#")) continue;
    const indented = /^[ \t]/.test(line);
    const colon = line.indexOf(":");
    if (colon < 0) return { ok: false, error: "badLine", detail: line.trim() };
    const key = line.slice(0, colon).trim();
    const value = line.slice(colon + 1);
    if (indented) {
      if (!inWhen) return { ok: false, error: "badLine", detail: line.trim() };
      const items = listValue(value);
      if (!items) return { ok: false, error: "badWhen", detail: key };
      if (key === "domains" || key === "apps" || key === "keywords") {
        draft[key] = items.join(", ");
      } else {
        return { ok: false, error: "badWhen", detail: key };
      }
      continue;
    }
    inWhen = false;
    if (key === "when") {
      if (value.trim()) return { ok: false, error: "badWhen", detail: key };
      inWhen = true;
    } else if (key === "name" || key === "description" || key === "version") {
      if (value.trim().startsWith("[")) return { ok: false, error: "badLine", detail: key };
      draft[key] = unquote(value);
    } else {
      return { ok: false, error: "unknownKey", detail: key };
    }
  }
  draft.body = lines
    .slice(end + 1)
    .join("\n")
    .trim();
  if (!draft.version.trim()) draft.version = DEFAULT_SKILL_VERSION;
  return { ok: true, draft };
}

// --- where the file goes ---------------------------------------------------

/** Path of the skill inside the daemon's data directory (shared scope). */
export function skillRelativePath(name: string): string {
  return `skills/${name}/SKILL.md`;
}

/**
 * The data directory from the doctor report (`GET /v1/doctor`, check `data_dir`): its detail is
 * `"<path> (writable)"`.  `null` when the report does not carry one.
 */
export function dataDirFromDoctor(
  report: { checks?: ReadonlyArray<{ id: string; detail: string }> } | null | undefined
): string | null {
  const detail = report?.checks?.find((c) => c.id === "data_dir")?.detail?.trim();
  if (!detail) return null;
  const path = detail.replace(/\s*\([^()]*\)\s*$/, "").trim();
  // Only accept something that looks like an absolute path.
  return /^([A-Za-z]:[\\/]|\/|\\\\)/.test(path) ? path : null;
}

/** The full file path to show, using the separator the data directory uses. */
export function skillInstallPath(dataDir: string | null, name: string): string {
  const rel = skillRelativePath(name);
  if (!dataDir) return rel;
  const sep = dataDir.includes("\\") ? "\\" : "/";
  return `${dataDir.replace(/[\\/]+$/, "")}${sep}${rel.split("/").join(sep)}`;
}

export type SkillCheck = "installed" | "rejected" | "notFound";

/**
 * After the user placed the file: is it in the real catalog now?  A load problem the daemon
 * reports for that folder (`issues`) beats "not found", so the user sees why it was refused.
 */
export function checkSkillInstalled(
  name: string,
  catalog: {
    skills: ReadonlyArray<{ name: string }>;
    issues: ReadonlyArray<{ path: string; message: string }>;
  }
): { state: SkillCheck; message?: string } {
  if (catalog.skills.some((s) => s.name === name)) return { state: "installed" };
  const issue = catalog.issues.find((i) =>
    i.path.replace(/\\/g, "/").split("/").includes(name)
  );
  return issue ? { state: "rejected", message: issue.message } : { state: "notFound" };
}
