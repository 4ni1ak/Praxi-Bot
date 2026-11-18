import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// The card cannot be rendered under `node --test` (no DOM), so this guards its
// wiring: every daemon call of the card goes through the one
// `DiagnosticsQueue` (the daemon answers 429 to overlapping diagnostics
// calls), and the header actions are disabled from the queue's task.

const src = readFileSync(new URL("./DiagnosticsCard.tsx", import.meta.url), "utf8");

/** Spans `[start, end)` of every `queue.current?.run(...)` call. */
function queueRuns(text: string): Array<[number, number]> {
  const spans: Array<[number, number]> = [];
  const marker = "queue.current?.run(";
  let from = 0;
  for (;;) {
    const start = text.indexOf(marker, from);
    if (start < 0) {
      return spans;
    }
    let depth = 0;
    let end = start + marker.length - 1;
    for (; end < text.length; end++) {
      if (text[end] === "(") depth++;
      if (text[end] === ")" && --depth === 0) break;
    }
    spans.push([start, end + 1]);
    from = end + 1;
  }
}

test("every daemon call of DiagnosticsCard runs inside the diagnostics queue", () => {
  assert.match(src, /new DiagnosticsQueue\(setBusy\)/);
  const runs = queueRuns(src);
  for (const task of ["load", "copy", "repair"]) {
    assert.ok(src.includes(`queue.current?.run("${task}"`), `no queued ${task}`);
  }
  const calls = [...src.matchAll(/client\.(doctor|doctorReport|doctorRepair)\(/g)];
  assert.equal(calls.length, 3, "expected doctor, doctorReport and doctorRepair calls");
  for (const call of calls) {
    const at = call.index ?? -1;
    assert.ok(
      runs.some(([s, e]) => at > s && at < e),
      `client.${call[1]}() is called outside the diagnostics queue`
    );
  }
});

test("Run again, Copy report and repairs are disabled from the queue's task", () => {
  assert.match(src, /const ctl = controls\(connected, busy\)/);
  assert.match(src, /disabled=\{ctl\.rerunDisabled/);
  assert.match(src, /disabled=\{ctl\.copyDisabled\}/);
  assert.match(src, /repairButton\(check, state, busy\)/);
});
