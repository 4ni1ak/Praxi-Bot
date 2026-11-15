import assert from "node:assert/strict";
import { test } from "node:test";
import { createPanelReserve } from "./panelReserve.ts";
import { reserveActive } from "./shellLayout.ts";

/** A manual clock: timers fire when `advance` passes their time. */
function harness(initialOpen: boolean, delay: number) {
  let now = 0;
  const timers = new Map<number, { at: number; fn: () => void }>();
  let id = 0;
  const changes: boolean[] = [];
  const ctl = createPanelReserve<number>({
    initialOpen,
    delayMs: () => delay,
    setTimer: (fn, ms) => {
      timers.set(++id, { at: now + ms, fn });
      return id;
    },
    clearTimer: (t) => void timers.delete(t),
    onChange: (v) => changes.push(v)
  });
  const advance = (ms: number) => {
    now += ms;
    for (const [k, t] of [...timers]) {
      if (t.at <= now) {
        timers.delete(k);
        t.fn();
      }
    }
  };
  return { ctl, advance, changes, pending: () => timers.size };
}

test("the reserve is kept for the delay after closing starts, then released", () => {
  const h = harness(true, 500);
  h.ctl.update(false);
  assert.equal(h.ctl.reserved, true, "still reserved right after closing starts");
  h.advance(499);
  assert.equal(h.ctl.reserved, true);
  h.advance(1);
  assert.equal(h.ctl.reserved, false);
  assert.deepEqual(h.changes, [false]);
});

test("the controller agrees with the model's reserveActive at every sampled time", () => {
  for (const t of [0, 1, 250, 449, 450, 499, 500, 501, 900]) {
    const h = harness(true, 500);
    h.ctl.update(false);
    h.advance(t);
    assert.equal(h.ctl.reserved, reserveActive(false, t, 500), `t=${t}`);
  }
});

test("reopening cancels the pending release; closing again restarts the full delay", () => {
  const h = harness(true, 500);
  h.ctl.update(false);
  h.advance(300);
  h.ctl.update(true);
  assert.equal(h.pending(), 0);
  h.advance(1000);
  assert.equal(h.ctl.reserved, true);
  h.ctl.update(false);
  h.advance(499);
  assert.equal(h.ctl.reserved, true);
  h.advance(1);
  assert.equal(h.ctl.reserved, false);
});

test("opening reserves immediately; reduced motion (delay 0) releases at once", () => {
  const h = harness(false, 500);
  h.ctl.update(true);
  assert.equal(h.ctl.reserved, true);
  const r = harness(true, 0);
  r.ctl.update(false);
  assert.equal(r.ctl.reserved, false, "no transition, no delay before the rail restores");
  assert.equal(r.pending(), 0);
});

test("dispose clears the pending timer; a repeated close does not restart it", () => {
  const h = harness(true, 500);
  h.ctl.update(false);
  h.advance(400);
  h.ctl.update(false);
  h.advance(100);
  assert.equal(h.ctl.reserved, false, "the first timer was not restarted");
  const d = harness(true, 500);
  d.ctl.update(false);
  d.ctl.dispose();
  assert.equal(d.pending(), 0);
  d.advance(1000);
  assert.equal(d.ctl.reserved, true);
});
