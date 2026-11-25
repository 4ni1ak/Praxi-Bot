import assert from "node:assert/strict";
import test from "node:test";
import { clockTime, minutes, newTrigger, triggerInput } from "./triggers.ts";

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
