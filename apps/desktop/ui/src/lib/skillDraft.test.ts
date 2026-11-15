import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  buildSkillMd,
  checkSkillInstalled,
  dataDirFromDoctor,
  emptySkillDraft,
  issueFor,
  parseSkillMd,
  SKILL_DESCRIPTION_MAX,
  SKILL_MAX_BYTES,
  SKILL_NAME_MAX,
  type SkillDraft,
  skillInstallPath,
  skillNameIssue,
  skillRelativePath,
  suggestSkillName,
  triggerList,
  validateSkillDraft
} from "./skillDraft.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");

function draft(over: Partial<SkillDraft> = {}): SkillDraft {
  return {
    ...emptySkillDraft(),
    name: "invoice-check",
    description: "Check an invoice against the order before paying.",
    body: "# Invoice check\n\n1. Open the order.\n2. Compare totals.",
    ...over
  };
}

test("skill names follow the daemon's validate_name rules", () => {
  assert.equal(skillNameIssue("github-basics"), null);
  assert.equal(skillNameIssue("a.b_c-1"), null);
  assert.equal(skillNameIssue(""), "nameEmpty");
  assert.equal(skillNameIssue("x".repeat(SKILL_NAME_MAX + 1)), "nameLength");
  assert.equal(skillNameIssue("x".repeat(SKILL_NAME_MAX)), null);
  assert.equal(skillNameIssue("GitHub"), "nameChars");
  assert.equal(skillNameIssue("my skill"), "nameChars");
  assert.equal(skillNameIssue("-lead"), "nameStart");
  assert.equal(skillNameIssue("a..b"), "nameDots");
  assert.equal(skillNameIssue("trail."), "nameDots");
});

test("a suggested name is always valid (or empty)", () => {
  assert.equal(suggestSkillName("GitHub İnceleme!"), "github-inceleme");
  assert.equal(suggestSkillName("  Şirket   Faturaları  "), "sirket-faturalari");
  assert.equal(suggestSkillName("...weird..name.."), "weird.name");
  assert.equal(suggestSkillName("!!!"), "");
  for (const title of ["GitHub İnceleme!", "a".repeat(200), "Çok — uzun: başlık / v2.0", "x..y"]) {
    const name = suggestSkillName(title);
    assert.equal(skillNameIssue(name), null, `${title} -> ${name}`);
  }
});

test("validation reports each rule on its field", () => {
  assert.deepEqual(validateSkillDraft(draft()), []);
  assert.equal(issueFor(validateSkillDraft(draft({ name: "Bad Name" })), "name"), "nameChars");
  assert.equal(
    issueFor(validateSkillDraft(draft(), ["invoice-check"]), "name"),
    "nameTaken"
  );
  assert.equal(
    issueFor(validateSkillDraft(draft({ description: " " })), "description"),
    "descriptionEmpty"
  );
  assert.equal(
    issueFor(
      validateSkillDraft(draft({ description: "x".repeat(SKILL_DESCRIPTION_MAX + 1) })),
      "description"
    ),
    "descriptionLong"
  );
  assert.equal(
    issueFor(validateSkillDraft(draft({ description: "x".repeat(SKILL_DESCRIPTION_MAX) })), "description"),
    null
  );
  assert.equal(
    issueFor(validateSkillDraft(draft({ description: "one\ntwo" })), "description"),
    "descriptionLines"
  );
  assert.equal(issueFor(validateSkillDraft(draft({ body: "  \n" })), "body"), "bodyEmpty");
  assert.equal(
    issueFor(validateSkillDraft(draft({ body: "x".repeat(SKILL_MAX_BYTES) })), "body"),
    "tooLarge"
  );
  assert.equal(issueFor(validateSkillDraft(draft({ version: "" })), "version"), "versionEmpty");
});

test("the built SKILL.md has the frontmatter shape the daemon parses", () => {
  const md = buildSkillMd(
    draft({ domains: "GitHub.com, example.org", keywords: "Fatura, invoice", apps: "" })
  );
  assert.equal(
    md,
    [
      "---",
      "name: invoice-check",
      'description: "Check an invoice against the order before paying."',
      'version: "0.1.0"',
      "when:",
      '  domains: ["github.com", "example.org"]',
      '  keywords: ["fatura", "invoice"]',
      "---",
      "# Invoice check",
      "",
      "1. Open the order.",
      "2. Compare totals.",
      ""
    ].join("\n")
  );
  // No triggers: no `when:` block at all.
  assert.ok(!buildSkillMd(draft()).includes("when:"));
});

test("build and parse round-trip, including awkward descriptions", () => {
  const cases: SkillDraft[] = [
    draft(),
    draft({ description: 'Says "hello": then [stops] # not a comment', apps: "Excel, Word" }),
    draft({ description: "'single quoted'", domains: "a.com", keywords: "x, y", version: "2.0" })
  ];
  for (const d of cases) {
    const parsed = parseSkillMd(buildSkillMd(d));
    assert.ok(parsed.ok, JSON.stringify(parsed));
    if (parsed.ok) {
      assert.equal(parsed.draft.name, d.name);
      assert.equal(parsed.draft.description, d.description);
      assert.equal(parsed.draft.version, d.version);
      assert.equal(parsed.draft.body, d.body.trim());
      assert.deepEqual(triggerList(parsed.draft.domains), triggerList(d.domains.toLowerCase()));
      assert.deepEqual(triggerList(parsed.draft.apps), triggerList(d.apps));
      assert.deepEqual(validateSkillDraft(parsed.draft), []);
    }
  }
});

test("a bundled skill file from the repo parses", () => {
  const parsed = parseSkillMd(read("../../../../../skills/github-basics/SKILL.md"));
  assert.ok(parsed.ok);
  if (parsed.ok) {
    assert.equal(parsed.draft.name, "github-basics");
    assert.equal(parsed.draft.domains, "github.com");
    assert.ok(parsed.draft.body.startsWith("# github.com basics"));
    assert.deepEqual(validateSkillDraft(parsed.draft), []);
  }
});

test("files the daemon would refuse are refused with a reason", () => {
  assert.deepEqual(parseSkillMd("# just markdown"), { ok: false, error: "noFrontmatter" });
  assert.deepEqual(parseSkillMd("---\nname: a\n"), { ok: false, error: "unterminated" });
  assert.deepEqual(parseSkillMd("---\nname: a\nauthor: me\n---\nbody"), {
    ok: false,
    error: "unknownKey",
    detail: "author"
  });
  assert.deepEqual(parseSkillMd("---\nname: a\nwhen: github\n---\nbody"), {
    ok: false,
    error: "badWhen",
    detail: "when"
  });
  assert.deepEqual(parseSkillMd("---\nwhen:\n  sites: [a]\n---\nbody"), {
    ok: false,
    error: "badWhen",
    detail: "sites"
  });
  assert.deepEqual(parseSkillMd("---\njust text\n---\nbody"), {
    ok: false,
    error: "badLine",
    detail: "just text"
  });
  assert.equal(parseSkillMd("x".repeat(SKILL_MAX_BYTES + 1)).ok, false);
  // A BOM and CRLF line ends are fine.
  const crlf = parseSkillMd("\uFEFF---\r\nname: a\r\ndescription: d\r\n---\r\nbody\r\n");
  assert.ok(crlf.ok);
  if (crlf.ok) assert.equal(crlf.draft.body, "body");
});

test("trigger lists drop characters the frontmatter list cannot carry", () => {
  assert.deepEqual(triggerList(' a.com , "b.com", [c], a.com,, '), ["a.com", "b.com", "c"]);
  assert.deepEqual(triggerList(""), []);
});

test("the install path comes from the doctor report when it carries a data dir", () => {
  assert.equal(skillRelativePath("invoice-check"), "skills/invoice-check/SKILL.md");
  const win = dataDirFromDoctor({
    checks: [{ id: "data_dir", detail: "C:\\Users\\me\\AppData\\Roaming\\praxi (writable)" }]
  });
  assert.equal(win, "C:\\Users\\me\\AppData\\Roaming\\praxi");
  assert.equal(
    skillInstallPath(win, "invoice-check"),
    "C:\\Users\\me\\AppData\\Roaming\\praxi\\skills\\invoice-check\\SKILL.md"
  );
  const unix = dataDirFromDoctor({ checks: [{ id: "data_dir", detail: "/tmp/x (writable)" }] });
  assert.equal(skillInstallPath(unix, "a"), "/tmp/x/skills/a/SKILL.md");
  // Nothing path-like: fall back to the relative path, never invent a directory.
  assert.equal(dataDirFromDoctor({ checks: [{ id: "data_dir", detail: "missing" }] }), null);
  assert.equal(dataDirFromDoctor({ checks: [] }), null);
  assert.equal(dataDirFromDoctor(null), null);
  assert.equal(skillInstallPath(null, "a"), "skills/a/SKILL.md");
});

test("the catalog re-check distinguishes installed, rejected and not found", () => {
  const catalog = {
    skills: [{ name: "github-basics" }],
    issues: [
      { path: "C:\\data\\skills\\broken\\SKILL.md", message: "missing required key `name`" }
    ]
  };
  assert.deepEqual(checkSkillInstalled("github-basics", catalog), { state: "installed" });
  assert.deepEqual(checkSkillInstalled("broken", catalog), {
    state: "rejected",
    message: "missing required key `name`"
  });
  assert.deepEqual(checkSkillInstalled("nope", catalog), { state: "notFound" });
  // A name that is only a substring of a path segment is not a match.
  assert.deepEqual(checkSkillInstalled("brok", catalog), { state: "notFound" });
});

// Wiring: the dialog validates and builds through this module and re-checks the REAL catalog;
// it must not pretend to install (no write route exists on the client).
test("AddSkillModal uses the tested module and the real catalog route", () => {
  const modal = read("../components/AddSkillModal.tsx");
  for (const fn of [
    "validateSkillDraft(",
    "buildSkillMd(",
    "parseSkillMd(",
    "checkSkillInstalled(",
    "skillInstallPath(",
    "dataDirFromDoctor("
  ]) {
    assert.ok(modal.includes(fn), `AddSkillModal uses ${fn}`);
  }
  assert.match(modal, /client\.listSkills\(\)/);
  const client = read("../api/client.ts");
  assert.doesNotMatch(client, /createSkill|importSkill|"POST", "\/skills"/);
});
