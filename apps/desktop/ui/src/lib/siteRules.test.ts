import assert from "node:assert/strict";
import test from "node:test";
import { siteRuleExpiryHours } from "./siteRules.ts";

test("expiry choices map to hours and never means no expiry", () => {
  assert.equal(siteRuleExpiryHours("never"), undefined);
  assert.equal(siteRuleExpiryHours("day"), 24);
  assert.equal(siteRuleExpiryHours("week"), 168);
  assert.equal(siteRuleExpiryHours("month"), 720);
});
