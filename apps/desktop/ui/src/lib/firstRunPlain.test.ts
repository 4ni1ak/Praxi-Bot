import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { FIRST_PROMPT_KEYS, keyShapeProblem } from "./firstRun.ts";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const locale = (lng: string): Record<string, Record<string, any>> =>
  JSON.parse(readFileSync(fileURLToPath(new URL(`../i18n/locales/${lng}.json`, import.meta.url)), "utf8"));

test("a key being typed gets plain advice before anything is sent", () => {
  assert.equal(keyShapeProblem("gemini", ""), null);
  assert.equal(keyShapeProblem("gemini", "https://aistudio.google.com/apikey"), "address");
  assert.equal(keyShapeProblem("gemini", "AIza short"), "address");
  assert.equal(keyShapeProblem("gemini", "AIzaSy123"), "short");
  assert.equal(keyShapeProblem("gemini", "sk-ant-api03-aaaaaaaaaaaaaaaaaaaaaaaa"), "prefix");
  assert.equal(keyShapeProblem("gemini", "AIza" + "A".repeat(35)), null);
  assert.equal(keyShapeProblem("anthropic", "sk-ant-api03-aaaaaaaaaaaaaaaaaaaaaaaa"), null);
  assert.equal(keyShapeProblem("ollama", "x"), null);
  assert.equal(keyShapeProblem("openai", "sk-proj-aaaaaaaaaaaaaaaaaaaaaaaaaaaa"), null);
});

test("the first-run flow ends with a Try this first card of four starter prompts in every language", () => {
  assert.equal(FIRST_PROMPT_KEYS.length, 4);
  const en = locale("en");
  for (const lng of ["en", "tr", "de"]) {
    const l = locale(lng);
    for (const key of FIRST_PROMPT_KEYS) {
      assert.ok(l.firstRun.prompts[key], `${lng} prompt ${key}`);
      if (lng !== "en") {
        assert.notEqual(l.firstRun.prompts[key], en.firstRun.prompts[key], `${lng} prompt ${key} translated`);
      }
    }
    assert.ok(l.firstRun.bot.tryTitle && l.firstRun.bot.whereBody && l.firstRun.bot.whereLater, `${lng} where/try copy`);
    assert.equal(l.botEmpty.suggestionsTitle, l.firstRun.bot.tryTitle);
    // The easy path: a free model with three steps, and a plain way to postpone.
    assert.equal(Object.keys(l.firstRun.brain.freeSteps).length, 3);
    assert.ok(l.firstRun.brain.laterNote.includes(l.firstRun.skip.replace(/[.]$/, "")), `${lng} later note names the skip button`);
  }
});

test("the wizard screens render the easy path, the where-it-runs note and the starter prompts", () => {
  const src = (f: string) => readFileSync(fileURLToPath(new URL(f, import.meta.url)), "utf8");
  assert.match(src("../views/firstrun/BrainStep.tsx"), /fr-gemini-steps/);
  assert.match(src("../views/firstrun/BrainStep.tsx"), /firstRun\.brain\.freeTitle/);
  assert.match(src("../views/firstrun/BotStep.tsx"), /fr-where/);
  assert.match(src("../views/firstrun/BotStep.tsx"), /FIRST_PROMPT_KEYS/);
});

test("the first screen offers three plain choices and keeps every other way to connect one click away", () => {
  const brain = readFileSync(fileURLToPath(new URL("../views/firstrun/BrainStep.tsx", import.meta.url)), "utf8");
  // The three first choices: what is already here, a free model, my own key.
  for (const id of ["fr-here", "fr-api-gemini", "fr-own"]) {
    assert.match(brain, new RegExp(`data-testid="${id}"`), id);
  }
  // Nothing was removed: providers with a key, Ollama, other local servers and each found app stay reachable.
  assert.match(brain, /\(\["anthropic", "openai"\] as const\)/);
  assert.match(brain, /data-testid="fr-api-ollama"/);
  assert.match(brain, /data-testid="fr-more"/);
  assert.match(brain, /data-testid=\{`fr-local-\$\{s\.id\}`\}/);
  assert.match(brain, /data-testid=\{`fr-cli-\$\{(single\.detected|d)\.id\}`\}/);
  // Technical words stay out of the first screen's own copy.
  const en = locale("en");
  const first = [en.firstRun.brain.title, en.firstRun.brain.subtitle, en.firstRun.steps.brain, en.firstRun.brain.here.title, en.firstRun.brain.own.title].join(" ");
  assert.doesNotMatch(first, /\bbrain\b/i);
  // The card grids never clip: they wrap by available width.
  const css = readFileSync(fileURLToPath(new URL("../views/firstrun/FirstRun.module.css", import.meta.url)), "utf8");
  for (const grid of ["cards", "tiles", "levels"]) {
    const block = new RegExp(`^\\.${grid} \\{[^}]*\\}`, "m").exec(css)?.[0] ?? "";
    assert.ok(block.includes("repeat(auto-fit, minmax(min(100%"), `${grid} wraps by available width`);
  }
  assert.doesNotMatch(css, /text-transform:\s*uppercase/);
});

test("the new first-run strings are really translated in every language", () => {
  const en = locale("en");
  const keys = [
    ["brain", "choices"],
    ["brain", "here", "title"],
    ["brain", "here", "sub"],
    ["brain", "own", "title"],
    ["brain", "own", "sub"],
    ["brain", "more", "toggle"],
    ["brain", "more", "hint"]
  ];
  const at = (l: Record<string, Record<string, unknown>>, path: string[]) =>
    path.reduce<unknown>((o, k) => (o as Record<string, unknown>)[k], l.firstRun);
  for (const lng of ["tr", "de", "es", "fr", "pt"]) {
    const l = locale(lng);
    for (const path of keys) {
      assert.ok(at(l, path), `${lng} ${path.join(".")}`);
      assert.notEqual(at(l, path), at(en, path), `${lng} ${path.join(".")} is translated`);
    }
  }
});
