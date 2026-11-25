import assert from "node:assert/strict";
import { test } from "node:test";
import { formatInScheduleZone, formatRunPreview, specTimeZone } from "./scheduleTime.ts";

test("run preview formats with a weekday in every UI locale and a time zone", () => {
  const ms = Date.UTC(2026, 10, 23, 6, 0);
  for (const locale of ["en", "tr", "de"]) {
    const text = formatRunPreview(ms, locale, "Europe/Istanbul");
    assert.match(text, /2026/);
    assert.match(text, /09[:.]00|9[:.]00/);
  }
});

const ZONE_MS = Date.UTC(2026, 6, 1, 6, 0, 0); // 09:00 in Istanbul (+03), 02:00 in New York

test("the zone is read from either cron spelling; specs without one have none", () => {
  assert.equal(specTimeZone({ cron: "0 9 * * * Europe/Istanbul" }), "Europe/Istanbul");
  assert.equal(
    specTimeZone({ cron: "CRON_TZ=Australia/Lord_Howe 0 9 * * *" }),
    "Australia/Lord_Howe"
  );
  assert.equal(specTimeZone({ cron: "0 9 * * *" }), undefined);
  assert.equal(specTimeZone({ every: { seconds: 60 } }), undefined);
  assert.equal(specTimeZone({ cron: "0 9 * * * Not/AZone" }), undefined);
});

test("run times show the schedule's own wall clock, not the viewer's", () => {
  const text = formatInScheduleZone(ZONE_MS, "en-US", "Europe/Istanbul", "America/New_York");
  assert.match(text, /9:00\s?AM/);
  assert.match(text, /GMT\+3/);
  const same = formatInScheduleZone(ZONE_MS, "en-US", "America/New_York", "America/New_York");
  assert.doesNotMatch(same, /EDT|GMT/);
});
