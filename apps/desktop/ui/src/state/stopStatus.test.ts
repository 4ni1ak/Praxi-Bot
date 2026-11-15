import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { RunEntry, RunPhase, StopRecord } from "./runLedger.ts";
import {
  describeStop,
  nextWidgetStop,
  STOPPED_VISIBLE_MS,
  type StopStatus,
  stopDetail,
  stopTitleKey,
  WIDGET_CONFIRMED_MS
} from "./stopStatus.ts";

function record(over: Partial<StopRecord> = {}): StopRecord {
  return {
    id: "s1",
    origin: "banner",
    kind: "all",
    nativeSeqs: [1],
    awaitingEcho: false,
    known: [],
    members: [],
    syncedAtDecision: true,
    decidedMono: 0,
    scope: "all_existing",
    phase: "in_flight",
    reached: false,
    firstSeen: null,
    audited: true,
    attempts: 1,
    retryIndex: 0,
    lastError: null,
    supersedes: null,
    successor: null,
    session: "s",
    settledMono: null,
    ...over
  };
}

function run(id: string, phase: RunPhase, error: string | null = null): RunEntry {
  return { id, botId: "b", phase, stopId: "s1", quarantined: false, processing: true, error };
}

const at = (records: StopRecord[], runs: RunEntry[], connected = true, now = 100) =>
  describeStop({ records, runs, now, connected });

// daemon outcome x native state -> what the user sees (the PR's state table)
test("state table: every daemon outcome maps to an honest status", () => {
  const two = ["a", "b"];
  const rows: [string, StopRecord[], RunEntry[], boolean, Partial<StopStatus> | null][] = [
    ["no client yet", [record({ members: two })], [run("a", "held"), run("b", "held")], false,
      { kind: "unreachable", tone: "warning", localHalted: true, dismissible: false }],
    ["transport error", [record({ members: two, lastError: "network", phase: "retrying" })], [run("a", "held"), run("b", "held")], true,
      { kind: "unreachable" }],
    ["sent, no answer yet", [record({ members: two })], [run("a", "held"), run("b", "held")], true,
      { kind: "stopping", tone: "progress" }],
    ["cancel_requested", [record({ members: two, reached: true, phase: "retrying" })], [run("a", "cancel_requested"), run("b", "stopped")], true,
      { kind: "stopping", stopped: 1, stopping: 1 }],
    ["failed member", [record({ members: two, reached: true, phase: "retrying" })], [run("a", "failed", "busy"), run("b", "stopped")], true,
      { kind: "partial", tone: "danger", failed: 1, error: "busy" }],
    ["all terminal", [record({ members: two, reached: true, phase: "settled", settledMono: 90 })], [], true,
      { kind: "stopped", tone: "success", stopped: 2, total: 2 }],
    ["nothing was running", [record({ reached: true, phase: "settled", settledMono: 90 })], [], true,
      { kind: "stopped", total: 0 }],
    ["audit row failed", [record({ reached: true, phase: "settled", settledMono: 90, audited: false })], [], true,
      { kind: "stopped", audited: false }],
    ["expired, successor working", [
      record({ id: "s1", members: ["a"], phase: "expired", successor: "s2", settledMono: 50 }),
      record({ id: "s2", members: ["a"], supersedes: "s1", scope: "listed" })
    ], [run("a", "held")], true, { kind: "expired", tone: "warning", stopId: "s1", dismissible: false }],
    ["expired, nothing held", [record({ phase: "expired", settledMono: 90, reached: false })], [], true,
      { kind: "expired", dismissible: true }],
    ["restored, unconfirmable", [record({ phase: "unconfirmable" })], [], false,
      { kind: "unconfirmable", tone: "warning" }]
  ];
  for (const [name, records, runs, connected, expected] of rows) {
    const s = at(records, runs, connected);
    assert.ok(s, name);
    for (const [k, v] of Object.entries(expected ?? {})) {
      assert.deepEqual((s as unknown as Record<string, unknown>)[k], v, `${name}: ${k}`);
    }
  }
});

test("cancel_requested is never counted as stopped", () => {
  const s = at([record({ members: ["a"], reached: true, phase: "retrying" })], [run("a", "cancel_requested")]);
  assert.equal(s?.stopped, 0);
  assert.notEqual(s?.kind, "stopped");
});

test("a confirmed Stop disappears after a short while; a dismissed one at once", () => {
  const settled = [record({ reached: true, phase: "settled", settledMono: 0 })];
  assert.equal(at(settled, [], true, STOPPED_VISIBLE_MS)?.kind, "stopped");
  assert.equal(at(settled, [], true, STOPPED_VISIBLE_MS + 1), null);
  assert.equal(describeStop({ records: settled, runs: [], now: 1, connected: true, dismissed: new Set(["s1"]) }), null);
});

test("single-run cancels from the chat are not shown as a global Stop", () => {
  assert.equal(at([record({ kind: "single" })], []), null);
});

test("every title and detail key the status can produce exists in en and tr", () => {
  const read = (l: string) =>
    JSON.parse(readFileSync(new URL(`../i18n/locales/${l}.json`, import.meta.url), "utf8")) as Record<string, unknown>;
  const has = (obj: Record<string, unknown>, key: string, count: number) => {
    const get = (k: string) => k.split(".").reduce<unknown>((o, p) => (o as Record<string, unknown> | undefined)?.[p], obj);
    return typeof get(key) === "string" || typeof get(`${key}_${count === 1 ? "one" : "other"}`) === "string";
  };
  const kinds = ["stopping", "unreachable", "partial", "expired", "unconfirmable", "stopped"] as const;
  for (const locale of ["en", "tr"]) {
    const dict = read(locale);
    for (const kind of kinds) {
      for (const total of [0, 1, 3]) {
        for (const held of [0, 2]) {
          const status: StopStatus = {
            stopId: "x", kind, tone: "progress", total, stopped: 0, stopping: held, failed: 0,
            localHalted: true, audited: true, dismissible: false, error: null
          };
          assert.ok(has(dict, stopTitleKey(status), total), `${locale}: ${stopTitleKey(status)}`);
          const d = stopDetail(status);
          assert.ok(has(dict, d.key, d.params.count ?? 0), `${locale}: ${d.key}`);
        }
      }
    }
    for (const key of ["stopStatus.notAudited", "stopStatus.dismiss", "stopStatus.progressLabel", "widget.stopPending", "widget.stopConfirmed"]) {
      assert.ok(has(dict, key, 0), `${locale}: ${key}`);
    }
  }
});

test("widget: pending while stopSeq > stopAckedSeq, confirmed briefly once acked", () => {
  let s = nextWidgetStop({ kind: "none" }, { stopSeq: 0, stopAckedSeq: 0 }, 0);
  assert.equal(s.kind, "none");
  s = nextWidgetStop(s, { stopSeq: 1, stopAckedSeq: 0 }, 10);
  assert.deepEqual(s, { kind: "pending", seq: 1 });
  s = nextWidgetStop(s, { stopSeq: 1, stopAckedSeq: 1 }, 20);
  assert.deepEqual(s, { kind: "confirmed", seq: 1, since: 20 });
  assert.equal(nextWidgetStop(s, { stopSeq: 1, stopAckedSeq: 1 }, 20 + WIDGET_CONFIRMED_MS).kind, "confirmed");
  assert.equal(nextWidgetStop(s, { stopSeq: 1, stopAckedSeq: 1 }, 21 + WIDGET_CONFIRMED_MS).kind, "none");
  // an older native build (no ack cursor) never shows a pending Stop
  assert.equal(nextWidgetStop({ kind: "none" }, { stopSeq: 3 }, 0).kind, "none");
});
