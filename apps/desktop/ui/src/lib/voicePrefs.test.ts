import assert from "node:assert/strict";
import { test } from "node:test";
import { parseVoiceRate, pickVoice } from "./voicePrefs.ts";

test("rate is clamped and bad values fall back to 1", () => {
  assert.equal(parseVoiceRate(9), 2);
  assert.equal(parseVoiceRate("fast"), 1);
  assert.equal(parseVoiceRate(1.23), 1.25);
});

test("pickVoice prefers the chosen voice, then a local one in the UI language", () => {
  const voices = [
    { name: "Online DE", lang: "de-DE", localService: false },
    { name: "Local DE", lang: "de_DE", localService: true },
    { name: "Local EN", lang: "en-US", localService: true }
  ];
  assert.equal(pickVoice(voices, "Local EN", "de")?.name, "Local EN");
  assert.equal(pickVoice(voices, "Gone", "de-AT")?.name, "Local DE");
  assert.equal(pickVoice(voices, "", "tr"), null);
});
