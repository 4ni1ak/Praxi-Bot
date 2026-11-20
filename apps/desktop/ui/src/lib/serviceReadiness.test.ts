import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { serviceIsUp, serviceReadiness } from "./serviceReadiness.ts";

// The same fixture the daemon-api and desktop (Rust) tests read.
const fixture = JSON.parse(
  readFileSync(
    new URL(
      "../../../../../crates/daemon-api/tests/fixtures/health-contract.json",
      import.meta.url
    ),
    "utf8"
  )
) as {
  cases: Array<{
    name: string;
    http_status: number | null;
    body: unknown;
    up: boolean;
    state: string;
  }>;
};

test("the health fixture covers every readiness state", () => {
  const states = new Set(fixture.cases.map((c) => c.state));
  for (const s of ["unreachable", "starting", "degraded", "setup_required", "ok"]) {
    assert.ok(states.has(s), `fixture lacks a ${s} case`);
  }
});

for (const c of fixture.cases) {
  test(`readiness of "${c.name}" follows the shared contract`, () => {
    const reply = { httpStatus: c.http_status, body: c.body };
    assert.equal(serviceReadiness(reply), c.state);
    assert.equal(serviceIsUp(reply), c.up);
  });
}

test("a first run with no provider is up, not a failure", () => {
  const r = { httpStatus: 200, body: { status: "setup_required", startup: "ready" } };
  assert.equal(serviceIsUp(r), true);
  assert.equal(serviceReadiness(r), "setup_required");
});
