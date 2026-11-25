import assert from "node:assert/strict";
import { test } from "node:test";
import { formatDay, formatTimeOfDay, formatWhen } from "./dateFormat.ts";

const NOW = Date.UTC(2026, 10, 25, 12, 0, 0);
const THIS_YEAR = Date.UTC(2026, 10, 25, 17, 9, 0);
const LAST_YEAR = Date.UTC(2025, 5, 15, 17, 9, 0);

test("this year shows no year; other years do", () => {
  const same = formatWhen(THIS_YEAR, "en", { zone: "UTC", now: NOW });
  assert.match(same, /^Nov 25, 5:09\s?PM$/);
  const other = formatWhen(LAST_YEAR, "en", { zone: "UTC", now: NOW });
  assert.match(other, /^Jun 15, 2025, 5:09\s?PM$/);
});

test("tr and de use the 24 hour clock", () => {
  assert.match(formatWhen(THIS_YEAR, "tr", { zone: "UTC", now: NOW }), /17[.:]09/);
  assert.match(formatWhen(THIS_YEAR, "de", { zone: "UTC", now: NOW }), /17[.:]09/);
  assert.match(formatTimeOfDay(THIS_YEAR, "de"), /^\d{2}[.:]\d{2}$/);
});

test("a foreign zone is named, the viewer's own is not", () => {
  const foreign = formatWhen(THIS_YEAR, "en", { zone: "Europe/Istanbul", zoneName: true, now: NOW });
  assert.match(foreign, /8:09\s?PM/);
  assert.match(foreign, /GMT\+3/);
  assert.doesNotMatch(formatWhen(THIS_YEAR, "en", { zone: "UTC", now: NOW }), /GMT|UTC/);
});

test("a day keeps the year only when it differs", () => {
  assert.equal(formatDay(THIS_YEAR, "en", NOW), "Nov 25");
  assert.equal(formatDay(LAST_YEAR, "en", NOW), "Jun 15, 2025");
});

test("a bad timestamp is an empty string, not 'Invalid Date'", () => {
  assert.equal(formatWhen(Number.NaN, "en"), "");
});
