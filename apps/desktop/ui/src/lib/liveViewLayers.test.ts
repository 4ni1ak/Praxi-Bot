import assert from "node:assert/strict";
import { test } from "node:test";
import { createInertTracker } from "./inertStack.ts";
import { viewerKeyAction } from "./liveViewLayers.ts";
import { createModalStack } from "./modalStack.ts";

const esc = { key: "Escape", shiftKey: false, defaultPrevented: false };
const tab = { key: "Tab", shiftKey: false, defaultPrevented: false };

test("the viewer owns Tab and Escape only while it is the topmost layer", () => {
  const stack = createModalStack();
  const viewer = stack.next();
  stack.add(viewer);
  const base = { stack, seq: viewer, interactive: false, stageFocused: false };
  assert.equal(viewerKeyAction({ ...base, event: tab }), "trap-tab");
  assert.equal(viewerKeyAction({ ...base, event: esc }), "close");

  // command palette / Popover / Modal registered above the viewer
  const palette = stack.next();
  stack.add(palette);
  assert.equal(viewerKeyAction({ ...base, event: tab }), null, "palette keeps Tab");
  assert.equal(viewerKeyAction({ ...base, event: esc }), null, "palette keeps Escape");

  stack.remove(palette);
  assert.equal(viewerKeyAction({ ...base, event: esc }), "close", "owns keys again");
});

test("an Escape another layer already handled never closes the viewer", () => {
  const stack = createModalStack();
  const viewer = stack.next();
  stack.add(viewer);
  const base = { stack, seq: viewer, interactive: false, stageFocused: false };
  assert.equal(
    viewerKeyAction({ ...base, event: { ...esc, defaultPrevented: true } }),
    null
  );
  assert.equal(
    viewerKeyAction({ ...base, event: { ...esc, shiftKey: true } }),
    null,
    "Shift+Esc is the stage focus exit"
  );
  assert.equal(viewerKeyAction({ ...base, interactive: true, event: esc }), null);
  assert.equal(viewerKeyAction({ ...base, stageFocused: true, event: esc }), null);
});

test("#root inertness is ref-counted: the app is interactive after any open/close order", () => {
  const orders: number[][] = [
    [1, -1],
    [1, 2, -1, -2],
    [1, 2, -2, -1],
    [1, 2, -1, 3, -3, -2]
  ];
  for (const order of orders) {
    const log: boolean[] = [];
    const tracker = createInertTracker((v) => log.push(v));
    for (const step of order) {
      if (step > 0) {
        tracker.acquire(step);
      } else {
        tracker.release(-step);
      }
    }
    assert.equal(log.at(-1), false, `order ${order.join(",")}`);
    assert.equal(tracker.holders, 0);
  }
  const log: boolean[] = [];
  const t = createInertTracker((v) => log.push(v));
  t.acquire(1);
  t.acquire(2);
  t.release(1);
  assert.equal(log.at(-1), true, "inert while any holder remains");
  t.release(2);
  assert.equal(log.at(-1), false);
  t.release(2);
  t.release(99);
  assert.equal(log.at(-1), false, "double or unknown release never leaves it inert");
});
