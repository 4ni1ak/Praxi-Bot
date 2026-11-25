import assert from "node:assert/strict";
import { test } from "node:test";
import {
  NO_FILTERS,
  RECENT_MAX,
  filterParams,
  filtersActive,
  loadRecent,
  pushRecent,
  resultDate,
  saveRecent,
  stepCursor
} from "./messageSearch.ts";

test("only the chips that narrow become query parameters", () => {
  assert.deepEqual(filterParams(NO_FILTERS, 1_000), {});
  assert.equal(filtersActive(NO_FILTERS), false);
  const f = { botId: "b1", when: "week" as const, hasFile: true, hasLink: false };
  assert.deepEqual(filterParams(f, 8 * 86_400_000), { bot_id: "b1", from_ms: 86_400_000, has_file: true });
  assert.equal(filtersActive(f), true);
});

test("recent searches are newest first, deduplicated across Turkish spellings and capped", () => {
  let list: string[] = [];
  list = pushRecent(list, "İstanbul");
  list = pushRecent(list, "rapor");
  list = pushRecent(list, "  istanbul  ");
  assert.deepEqual(list, ["istanbul", "rapor"]);
  for (let i = 0; i < 10; i++) list = pushRecent(list, `q${i}`);
  assert.equal(list.length, RECENT_MAX);
  assert.equal(list[0], "q9");
  assert.deepEqual(pushRecent(list, "   "), list);
});

test("recent searches survive a round trip and ignore damaged storage", () => {
  const mem = new Map<string, string>();
  const s = {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k)
  };
  saveRecent(["a", "b"], s);
  assert.deepEqual(loadRecent(s), ["a", "b"]);
  saveRecent([], s);
  assert.deepEqual(loadRecent(s), []);
  mem.set("praxi.recentSearches", "{not json");
  assert.deepEqual(loadRecent(s), []);
  mem.set("praxi.recentSearches", JSON.stringify(["ok", 5, ""]));
  assert.deepEqual(loadRecent(s), ["ok"]);
});

test("arrow keys wrap and handle an empty list", () => {
  assert.equal(stepCursor(-1, 0, 1), -1);
  assert.equal(stepCursor(-1, 3, 1), 0);
  assert.equal(stepCursor(-1, 3, -1), 2);
  assert.equal(stepCursor(2, 3, 1), 0);
  assert.equal(stepCursor(0, 3, -1), 2);
});

test("dates read short: time today, day and month otherwise", () => {
  const now = new Date(2026, 10, 24, 15, 0).getTime();
  assert.match(resultDate(new Date(2026, 10, 24, 9, 5).getTime(), now, "en"), /9:05|09:05/);
  assert.doesNotMatch(resultDate(new Date(2026, 9, 3).getTime(), now, "en"), /2026/);
  assert.match(resultDate(new Date(2025, 9, 3).getTime(), now, "en"), /2025/);
});
