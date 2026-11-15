import { foldText } from "./searchText.ts";

/**
 * The "Add skill" flow (owner feedback 244, point 4): compose a `SKILL.md` from text or read an
 * existing one.
 *
 * The checks here are a PRE-CHECK only: a local mirror of what the daemon's `skills` crate
 * (`skill.rs`, `frontmatter.rs`) accepts, kept close by shared fixtures in the tests, to spare the
 * user an obvious round trip.  The daemon has no route that validates or writes a skill (`GET
 * /v1/skills`, `GET|PUT /v1/bots/{id}/skills` only), so the only truth is its catalog after the
 * file is in place - `checkSkillInstalled` - which also reports why it refused a folder.
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

/**
 * Split a list of triggers typed as text: comma or newline separated, an item that itself holds a
 * comma is written in double quotes (`"hello, world", other`) - the same shape the frontmatter
 * list uses.  Double quotes and brackets cannot be part of an item.
 */
export function triggerList(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  const parts: string[] = [];
  let cur = "";
  let quoted = false;
  for (const c of text) {
    if (c === '"') {
      quoted = !quoted;
    } else if ((c === "," || c === "\n") && !quoted) {
      parts.push(cur);
      cur = "";
    } else {
      cur += c;
    }
  }
  parts.push(cur);
  for (const part of parts) {
    const item = part.replace(/[[\]]/g, "").trim();
    if (item && !seen.has(item)) {
      seen.add(item);
      out.push(item);
    }
  }
  return out;
}

/** Items as the text a trigger field shows (the inverse of `triggerList`). */
export function formatTriggers(items: readonly string[]): string {
  return items.map((i) => (/[,\n]/.test(i) ? `"${i}"` : i)).join(", ");
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
  | "duplicateKey"
  | "emptyVersion"
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

/** An inline list the way `frontmatter.rs` reads it: commas inside quotes do not split. */
function listValue(raw: string): string[] | null {
  const s = raw.trim();
  if (!s.startsWith("[") || !s.endsWith("]")) return null;
  const items: string[] = [];
  let cur = "";
  let quote: string | null = null;
  for (const c of s.slice(1, -1)) {
    if (quote) {
      if (c === quote) quote = null;
      cur += c;
    } else if (c === '"' || c === "'") {
      quote = c;
      cur += c;
    } else if (c === ",") {
      items.push(cur);
      cur = "";
    } else {
      cur += c;
    }
  }
  if (quote) return null;
  items.push(cur);
  return items.map((i) => unquote(i)).filter(Boolean);
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
  const seenTop = new Set<string>();
  const seenWhen = new Set<string>();
  for (const line of lines.slice(1, end)) {
    if (!line.trim() || line.trimStart().startsWith("#")) continue;
    const indented = /^[ \t]/.test(line);
    const colon = line.indexOf(":");
    if (colon < 0) return { ok: false, error: "badLine", detail: line.trim() };
    const key = line.slice(0, colon).trim();
    const value = line.slice(colon + 1);
    if (indented) {
      if (!inWhen) return { ok: false, error: "badLine", detail: line.trim() };
      if (seenWhen.has(key)) return { ok: false, error: "duplicateKey", detail: key };
      seenWhen.add(key);
      const items = listValue(value);
      if (!items) return { ok: false, error: "badWhen", detail: key };
      if (key === "domains" || key === "apps" || key === "keywords") {
        draft[key] = formatTriggers(items);
      } else {
        return { ok: false, error: "badWhen", detail: key };
      }
      continue;
    }
    inWhen = false;
    if (seenTop.has(key)) return { ok: false, error: "duplicateKey", detail: key };
    seenTop.add(key);
    if (key === "when") {
      if (value.trim()) return { ok: false, error: "badWhen", detail: key };
      inWhen = true;
    } else if (key === "name" || key === "description" || key === "version") {
      if (value.trim().startsWith("[")) return { ok: false, error: "badLine", detail: key };
      draft[key] = unquote(value);
      // `skill.rs`: an explicit `version:` must not be empty (only an absent one defaults).
      if (key === "version" && !draft.version.trim()) return { ok: false, error: "emptyVersion" };
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

/**
 * Whether closing the add-skill dialog loses something: anything the user wrote is unsaved until
 * the daemon's catalog lists that file.  The step (compose or install) does not matter: moving to
 * the install step stores nothing.
 */
export function skillDraftDirty(s: { touched: boolean; installed: boolean }): boolean {
  return s.touched && !s.installed;
}

/** Hex SHA-256 of the instructions as the daemon stores them (the trimmed body). */
export async function skillBodyHash(body: string): Promise<string> {
  const bytes = new TextEncoder().encode(body.replace(/\r\n/g, "\n").trim());
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** What the add-skill dialog remembers about the daemon check of its current draft. */
export interface CheckResult {
  state: SkillCheck | "failed";
  message?: string;
  existing?: CatalogSkill;
  verified?: boolean;
}

/** The add-skill dialog's transient state: edits, the check of the current draft, the assignment. */
export interface AddFlow {
  touched: boolean;
  check: CheckResult | null;
  assigned: boolean;
}

export const INITIAL_ADD_FLOW: AddFlow = { touched: false, check: null, assigned: false };

/**
 * Any change to the draft (typing, importing another file) discards the check and the assignment
 * result: they belong to the exact draft they were made for.
 */
export function flowAfterEdit(): AddFlow {
  return { touched: true, check: null, assigned: false };
}

export function flowAfterCheck(f: AddFlow, check: CheckResult): AddFlow {
  return { ...f, check };
}

/** Closing loses the draft unless the daemon's catalog verifiably lists this very file. */
export function flowDirty(f: AddFlow): boolean {
  return skillDraftDirty({
    touched: f.touched,
    installed: f.check?.state === "installed" && f.check.verified === true
  });
}

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

export type SkillCheck = "installed" | "rejected" | "notFound" | "different";

/** What the catalog (`GET /v1/skills`) says about one skill: the fields the API exposes. */
export interface CatalogSkill {
  name: string;
  description?: string;
  version?: string;
  /** Hex SHA-256 of the body the daemon loaded (`GET /v1/skills`); absent on an older daemon. */
  body_sha256?: string;
  scope?: string;
  requires?: {
    domains?: readonly string[];
    apps?: readonly string[];
    keywords?: readonly string[];
  } | null;
}

const lowerSorted = (items: readonly string[] | undefined): string =>
  [...(items ?? [])]
    .map((i) => i.toLowerCase())
    .sort()
    .join("\u0000");

/**
 * Whether a catalog entry is the file the user supplied: the API has no content hash, so the
 * identity is the name plus everything the catalog carries from the file - version, description
 * and the `when` triggers - in the shared scope (a bundled skill of the same name is not it) -
 * and, when the daemon reports it, the SHA-256 of the instructions.
 */
export function catalogEntryMatches(
  entry: CatalogSkill,
  draft: SkillDraft,
  bodyHash?: string
): boolean {
  if (entry.body_sha256 !== undefined && bodyHash !== undefined && entry.body_sha256 !== bodyHash) {
    return false;
  }
  return (
    entry.name === draft.name.trim() &&
    (entry.scope ?? "shared") === "shared" &&
    entry.version === (draft.version.trim() || DEFAULT_SKILL_VERSION) &&
    (entry.description ?? "") === draft.description.replace(/\r?\n/g, " ").trim() &&
    lowerSorted(entry.requires?.domains) === lowerSorted(triggerList(draft.domains)) &&
    lowerSorted(entry.requires?.apps) === lowerSorted(triggerList(draft.apps)) &&
    lowerSorted(entry.requires?.keywords) === lowerSorted(triggerList(draft.keywords))
  );
}

/**
 * After the user placed the file: did the daemon load THIS file?
 *
 * 1. a load problem the daemon reports for that folder (`issues`) always wins - an older skill of
 *    the same name must never hide the refusal of the file just supplied;
 * 2. a catalog entry equal to the file in name, version, description and triggers = installed;
 * 3. a different entry of that name = `different` (an older or bundled skill; not this file);
 * 4. nothing = not found.
 */
export function checkSkillInstalled(
  draft: SkillDraft,
  catalog: {
    skills: ReadonlyArray<CatalogSkill>;
    issues: ReadonlyArray<{ path: string; message: string; scope?: string }>;
  },
  bodyHash?: string
): { state: SkillCheck; message?: string; existing?: CatalogSkill; verified?: boolean } {
  const name = draft.name.trim();
  const issue = catalog.issues.find((i) => {
    if (i.scope === "bundled") return false;
    const parts = i.path.replace(/\\/g, "/").split("/");
    const at = parts.lastIndexOf(name);
    return at > 0 && parts[at - 1] === "skills";
  });
  if (issue) return { state: "rejected", message: issue.message };
  const existing = catalog.skills.find((s) => s.name === name);
  if (!existing) return { state: "notFound" };
  if (!catalogEntryMatches(existing, draft, bodyHash)) return { state: "different", existing };
  // Without the daemon's body hash (an older daemon) a same-metadata older file cannot be told
  // apart from this one: the result stands, but it is not "verified" and the guard stays on.
  return {
    state: "installed",
    existing,
    verified: existing.body_sha256 !== undefined && bodyHash !== undefined
  };
}
