import assert from "node:assert/strict";
import { test } from "node:test";
import type { HostPackInfo, HostSetupJob } from "../api/types";
import {
  BASE_PACKS,
  BASE_SET_MB,
  jobFailure,
  licenseIsMixed,
  missingBase,
  needsAck,
  reasonKey,
  reasonOf,
  rowStates,
  safeSourceUrl,
  sizeParts,
  sortedPacks,
  totalMb
} from "./toolPacks.ts";

const pack = (id: string, status: string, base = true, size_mb = 100): HostPackInfo => ({
  id,
  status,
  size_mb,
  base,
  removable: id !== "dev",
  license: "Apache-2.0",
  copyleft: false,
  distro: true,
  needs_ack: false,
  source_url: "https://example.org/src",
  notice_url: "https://example.org/licence",
  version: "",
  lock_sha256: null
});

const job = (state: HostSetupJob["state"], steps: Array<[string, string, string]>): HostSetupJob =>
  ({
    id: "j",
    kind: "packs",
    state,
    target: "u@h:22",
    steps: steps.map(([id, status, detail]) => ({ id, title: id, status, detail, hint: null })),
    notes: [],
    host_key: null,
    host_id: null,
    host_name: null,
    view_targets: null,
    install_available: true
  }) as HostSetupJob;

test("sizes read as MB under a gigabyte and as GB with one decimal above", () => {
  assert.deepEqual(sizeParts(700), { value: "700", unit: "MB" });
  assert.deepEqual(sizeParts(1240), { value: "1.2", unit: "GB" });
  assert.deepEqual(sizeParts(2000), { value: "2", unit: "GB" });
  assert.equal(totalMb([pack("office", "missing", true, 700), pack("docs", "missing", true, 150)], ["office", "docs"]), 850);
});

test("packs keep the shown order and an unknown id from a newer daemon is kept last", () => {
  const ids = sortedPacks([pack("dev", "missing"), pack("future", "missing"), pack("office", "missing")]).map((p) => p.id);
  assert.deepEqual(ids, ["office", "dev", "future"]);
});

test("the basic set only offers base packs that are not complete", () => {
  const packs = [pack("office", "installed"), pack("docs", "partial"), pack("ocr", "missing", false), pack("media", "missing")];
  assert.deepEqual(missingBase(packs), ["docs", "media"]);
});

test("rows: finished steps are done or failed, the next pack runs, the rest wait, a failure stops the line", () => {
  const ids = ["office", "docs", "media"];
  assert.deepEqual(rowStates(job("running", [["pack.office", "pass", "done"]]), ids), {
    office: "done",
    docs: "running",
    media: "pending"
  });
  assert.deepEqual(rowStates(job("failed", [["pack.office", "pass", "done"], ["pack.docs", "fail", "failed"]]), ids), {
    office: "done",
    docs: "failed",
    media: "pending"
  });
  assert.equal(rowStates(null, ids).office, "running");
});

test("reasons: unknown codes read as a generic problem and plan and run names agree", () => {
  assert.equal(reasonOf("needs_admin"), "needs_admin");
  assert.equal(reasonOf("rm -rf /"), "failed");
  assert.equal(reasonOf(null), "failed");
  assert.equal(reasonKey("blocked_shared"), "shared");
  assert.equal(reasonKey("dependants"), "dependants");
  assert.equal(jobFailure(job("failed", [["pack.office", "fail", "needs_admin"]])), "needs_admin");
  assert.equal(jobFailure(job("succeeded", [["pack.office", "pass", "done"]])), null);
});

test("the wizard switch names every base pack and its sizes add up to the shown total", () => {
  assert.deepEqual(
    BASE_PACKS.map((p) => p.id),
    ["office", "docs", "media", "dev"]
  );
  assert.equal(BASE_SET_MB, 1350);
});

test("the scraping pack is listed last and its reasons have words", () => {
  assert.deepEqual(sortedPacks([pack("scrape", "missing"), pack("office", "missing")]).map((p) => p.id), ["office", "scrape"]);
  assert.equal(reasonKey("needs_python"), "needs_python");
  assert.equal(reasonKey("needs_ack"), "needs_ack");
});

test("licence chips: a mix of licences collapses to one word, links must be https", () => {
  assert.equal(licenseIsMixed("Apache-2.0"), false);
  assert.equal(licenseIsMixed("GPL-2.0-only AND MIT"), true);
  assert.equal(safeSourceUrl("https://github.com/adbar/trafilatura"), "https://github.com/adbar/trafilatura");
  assert.equal(safeSourceUrl("javascript:alert(1)"), null);
  assert.equal(safeSourceUrl("http://plain.example"), null);
});

test("a copyleft download needs the acknowledgement before it can be reviewed", () => {
  const agpl = { ...pack("agpl", "missing", false), license: "AGPL-3.0-only", copyleft: true, distro: false, needs_ack: true };
  const packs = [pack("office", "missing"), agpl];
  assert.deepEqual(needsAck(packs, ["office"]).map((p) => p.id), []);
  assert.deepEqual(needsAck(packs, ["office", "agpl"]).map((p) => p.id), ["agpl"]);
});
