import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { installedReleaseNotes, notesFor, whatsNewAction, type WhatsNewFile } from "./whatsNew.ts";

const read = (rel: string) => JSON.parse(readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8"));

test("the bundled notes cover the shipped version in en, tr and de", () => {
  const version = read("../../../tauri.conf.json").version as string;
  const file = read("../whats-new.json") as WhatsNewFile;
  const titles = new Set<string>();
  for (const lang of ["en", "tr", "de"] as const) {
    const own = file[version]?.[lang];
    assert.ok(own && own.title && own.items.length > 0, `${lang} notes for ${version}`);
    titles.add(own.title);
  }
  assert.equal(titles.size, 3, "each language has its own text");
});

test("notes fall back to English, and an unknown version has none", () => {
  const file: WhatsNewFile = { "1.0.0": { en: { title: "E", items: ["a"] } } };
  assert.equal(notesFor(file, "1.0.0", "de-DE")?.title, "E");
  assert.equal(notesFor(file, "2.0.0", "en"), null);
});

test("show once after an update; a fresh install or a note-less version only records", () => {
  const base = { version: "1.1.0", lastSeen: "1.0.0", onboardingCompleted: true, hasNotes: true };
  assert.equal(whatsNewAction(base), "show");
  assert.equal(whatsNewAction({ ...base, lastSeen: "1.1.0" }), "none");
  assert.equal(whatsNewAction({ ...base, lastSeen: "", onboardingCompleted: false }), "record");
  assert.equal(whatsNewAction({ ...base, lastSeen: "", onboardingCompleted: true }), "show");
  assert.equal(whatsNewAction({ ...base, hasNotes: false }), "record");
  assert.equal(whatsNewAction({ ...base, version: "" }), "none");
});

test("installed feed notes are inert text for the running version and shown once", () => {
  const body = '<img src=x onerror="alert(1)">\n[click](javascript:alert(1))';
  const notes = { version: "2.0.0", notes: body };
  const selected = installedReleaseNotes(notes, "2.0.0", "What's new");
  assert.deepEqual(selected, { title: "What's new", items: [], text: body });
  assert.equal(installedReleaseNotes(notes, "1.0.0", "What's new"), null);
  assert.equal(installedReleaseNotes({ ...notes, notes: " " }, "2.0.0", "What's new"), null);
  const args = { version: "2.0.0", lastSeen: "1.0.0", onboardingCompleted: true, hasNotes: selected !== null };
  assert.equal(whatsNewAction(args), "show");
  assert.equal(whatsNewAction({ ...args, lastSeen: "2.0.0" }), "none");
});
