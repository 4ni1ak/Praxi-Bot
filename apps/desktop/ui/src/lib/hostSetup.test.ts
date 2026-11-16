import assert from "node:assert/strict";
import { test } from "node:test";
import type { HostSetupJob, HostSetupStep } from "../api/types";
import { copyDetails, failure, stageStates, wizardView } from "./hostSetup.ts";

const step = (id: string, status: HostSetupStep["status"], detail = ""): HostSetupStep => ({
  id,
  title: id,
  status,
  detail,
  hint: null
});

const job = (state: HostSetupJob["state"], steps: HostSetupStep[]): HostSetupJob => ({
  id: "j1",
  state,
  target: "ubuntu@203.0.113.7:22",
  steps,
  notes: [],
  host_key: null,
  host_id: null,
  host_name: null,
  view_targets: null,
  install_available: false
});

test("rows follow what the engine reported, and the open row is honest about waiting", () => {
  assert.deepEqual(Object.values(stageStates(job("awaiting_host_key", []))), [
    "waiting",
    "pending",
    "pending",
    "pending",
    "pending"
  ]);
  const mid = stageStates(
    job("running", [
      step("hostkey", "pass"),
      step("connect", "pass"),
      step("preflight.os", "pass"),
      step("preflight.disk", "pass")
    ])
  );
  assert.deepEqual(Object.values(mid), ["done", "running", "pending", "pending", "pending"]);
});

test("an already installed server shows install as skipped and the run as done", () => {
  const states = stageStates(
    job("succeeded", [
      step("hostkey", "pass"),
      step("connect", "pass"),
      step("preflight.os", "pass"),
      step("install", "skip"),
      step("pair", "pass"),
      step("health.channel", "pass")
    ])
  );
  assert.deepEqual(Object.values(states), ["done", "done", "skipped", "done", "done"]);
  assert.equal(wizardView(job("succeeded", [])), "done");
});

test("a failure maps to one code, stops later rows, and the details carry no secrets", () => {
  const j = job("failed", [
    step("hostkey", "pass"),
    step("connect", "fail", "the server rejected the login (key or user)")
  ]);
  assert.equal(failure(j)?.code, "auth");
  assert.deepEqual(Object.values(stageStates(j)), [
    "failed",
    "pending",
    "pending",
    "pending",
    "pending"
  ]);
  assert.equal(wizardView(j), "failed");
  assert.equal(
    failure(job("failed", [step("install", "fail", "Praxi is not installed on this server")]))?.code,
    "not_installed"
  );
  assert.equal(
    failure(
      job("failed", [
        step("hostkey", "fail", "connected, but could not read the server's host key: choose_kex")
      ])
    )?.code,
    "hostkey_unreadable"
  );
  assert.match(copyDetails(j), /\[fail\] connect: the server rejected the login/);
});
