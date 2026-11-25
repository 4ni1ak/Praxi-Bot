import assert from "node:assert/strict";
import test from "node:test";
import { clockTime, dailyTime, minutes, newTrigger, triggerInput, triggerSpec } from "./triggers.ts";
import type { ScheduleSpec } from "./schedules.ts";

test("rules start off with bounded polling and visible safe defaults", () => {
  const rule = newTrigger();
  assert.equal(rule.enabled, false);
  assert.equal(rule.config.source.kind === "mail" && rule.config.source.poll_seconds, 120);
  assert.deepEqual(rule.config.limits, { per_hour: 10, per_day: 50 });
  const edited = triggerInput({ ...rule, id: "runtime-id", origin: "rule", count_today: 8 } as typeof rule);
  assert.equal("id" in edited, false);
  assert.equal("origin" in edited, false);
  edited.config.limits.per_hour = 2;
  assert.equal(rule.config.limits.per_hour, 10);
  assert.equal(clockTime(minutes("22:05")), "22:05");
});

test("saved intervals, one-time and weekday rules never masquerade as daily rules", () => {
  const specs: ScheduleSpec[] = [
    { every: { seconds: 3600 } },
    { at: { unix_ms: 1_800_000_000_000 } },
    { cron: "0 9 * * 1-5 Europe/Berlin" }
  ];
  for (const spec of specs) {
    assert.equal(dailyTime({ kind: "time", spec }), null);
    assert.deepEqual(triggerSpec("cron" in spec ? spec.cron : JSON.stringify(spec)), spec);
  }
  assert.deepEqual(dailyTime({ kind: "time", spec: { cron: "5 9 * * * Europe/Berlin" } }),
    { time: "09:05", zone: "Europe/Berlin" });
});
