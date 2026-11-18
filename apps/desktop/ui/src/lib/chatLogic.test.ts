import assert from "node:assert/strict";
import { test } from "node:test";
import {
  changesSectionVisible,
  nextFollow,
  shouldProbeChanges,
  showJumpToLatest
} from "./chatLogic.ts";

test("the Changes section needs a finished run and at least one changed file", () => {
  const tool = { kind: "tool", name: "fs_write" };
  const search = { kind: "tool", name: "github.search" };
  assert.equal(shouldProbeChanges({ items: [tool] }, true), false, "still running");
  assert.equal(shouldProbeChanges({ finishedMs: 1, items: [search] }, false), false);
  assert.equal(shouldProbeChanges({ finishedMs: 1, items: [tool] }, false), true);
  assert.equal(shouldProbeChanges({ finishedMs: 1, items: [search] }, true), true);
  assert.equal(changesSectionVisible(null), false);
  assert.equal(changesSectionVisible(0), false);
  assert.equal(changesSectionVisible(2), true);
});

test("scrolling up detaches at once, reaching the end re-attaches, growth keeps state", () => {
  const at = (scrollTop: number) => ({ scrollHeight: 2000, clientHeight: 600, scrollTop });
  assert.equal(nextFollow(true, at(1350), 1355), false, "5px up away from the end is intent");
  assert.equal(nextFollow(true, at(1395), 1400), true, "within the end zone it keeps following");
  assert.equal(nextFollow(false, at(1200), 1190), false, "stays detached");
  assert.equal(nextFollow(false, at(1390), 1200), true, "back at the end");
  assert.equal(nextFollow(true, at(1300), 1300), true, "content grew below, no scroll");
  assert.equal(nextFollow(false, at(900), 900), false);
});

test("jump to latest only while detached and clearly away from the end", () => {
  const m = (scrollTop: number) => ({ scrollHeight: 2000, clientHeight: 600, scrollTop });
  assert.equal(showJumpToLatest(true, m(0)), false);
  assert.equal(showJumpToLatest(false, m(1380)), false, "almost at the end");
  assert.equal(showJumpToLatest(false, m(800)), true);
});
