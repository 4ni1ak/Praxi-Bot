import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { hostRefusalKey, onboardingAllowed } from "./hostMode.ts";
import { HOST_STATE_HOLD_MS, settleHostState } from "./hostState.ts";

const flat = (file: string): Record<string, unknown> => {
  const out: Record<string, unknown> = {};
  const walk = (o: Record<string, unknown>, prefix: string) => {
    for (const [k, v] of Object.entries(o)) {
      if (v && typeof v === "object") walk(v as Record<string, unknown>, `${prefix}${k}.`);
      else out[`${prefix}${k}`] = v;
    }
  };
  walk(JSON.parse(readFileSync(new URL(`../i18n/locales/${file}.json`, import.meta.url), "utf8")), "");
  return out;
};

test("first-run wizard is not shown when a host is selected", () => {
  assert.equal(onboardingAllowed("local"), true);
  assert.equal(onboardingAllowed("ca99609c0000000000000000000000000000000000000000000000000000abcd"), false);
});

test("every host refusal maps to a translated sentence, never the raw daemon text", () => {
  const daemon = [
    "the execution policy is managed on the host itself",
    "secrets are managed on the host itself",
    "tool servers and skills are managed on the host itself",
    "account credentials are managed on the host itself",
    "host transfer and channel pairing are done on the host itself",
    "pairing and host lifecycle are managed on the host itself",
    "permissions and tools of a bot on a host are managed on the host itself",
    "knowledge settings are managed by the local owner",
    "Composio connections are managed by the owner's desktop only",
    "only the local user answers overlay questions",
    "schedule triggers and prechecks are local-owner only",
    "peer role does not allow this operation",
    "host-local provider management is not allowed for peers",
    "connection tests spend this host's stored keys and are an owner action",
    "this needs the owner key: start Praxi from the desktop app, or run it on the host with its owner key",
    "recent logs are available only to the local owner",
    "the audit log is available only to the local owner",
    "the outgoing-mail guard (send log, opt-out list, limits) is an owner action on this host",
    "bot profile files are handled by the owner on this host only",
    "embedding providers and their keys are a host-local owner setting",
    "trusted folders are a host-local owner setting",
    "personal memory is managed by the local owner",
    "a bot's spending limits can only be changed at this computer",
    "MCP server management is only allowed for the local user",
    "host links are managed by the local user only",
    "the MCP server facade is host-local and requires scoped credentials",
    "host-local diagnostics are not allowed for peers",
    "the signed ingress of an incoming webhook is not a peer path",
    "screen permissions of devices are granted on the host itself"
  ];
  const files = ["en", "tr", "de", "es", "fr", "pt"].map((f) => [f, flat(f)] as const);
  for (const message of daemon) {
    const key = hostRefusalKey(403, message);
    assert.ok(key && key !== "generic", message);
    for (const [file, texts] of files) {
      assert.ok(typeof texts[`hostMode.refusal.${key}`] === "string", `${file} ${key}`);
    }
  }
  // The stable code counts as much as the text (an older or newer daemon may word it differently).
  assert.equal(hostRefusalKey(403, "whatever", "owner_key_required"), "needsAdmin");
  assert.equal(hostRefusalKey(403, "something new"), "generic");
  // The catch-all never shows the daemon's English sentence: it says what happened and where the detail is.
  for (const [file, texts] of files) {
    const generic = texts["hostMode.refusal.generic"] as string;
    assert.ok(!generic.includes("{{reason}}"), file);
    assert.ok(generic.includes("{{name}}"), file);
  }
});

test("a refusal that the confirmed desktop can fix tells what to run and where, in every language", () => {
  for (const file of ["en", "tr", "de", "es", "fr", "pt"]) {
    for (const key of ["adminRequired", "needsAdmin"]) {
      const text = flat(file)[`hostMode.refusal.${key}`] as string;
      assert.ok(text.includes("bot host admin --all-desktops"), `${file} ${key}`);
    }
  }
});

test("an unconfirmed desktop gets an actionable sentence with the command, not a dead end", () => {
  const daemon = "the owner must confirm this desktop on the host: run `bot host admin --all-desktops`";
  assert.equal(hostRefusalKey(403, daemon), "adminRequired");
  for (const file of ["en", "tr", "de", "es", "fr", "pt"]) {
    const text = flat(file)["hostMode.refusal.adminRequired"];
    assert.ok(typeof text === "string" && text.includes("bot host admin --all-desktops"), file);
    assert.ok((text as string).includes("{{name}}"), file);
  }
});

test("a good host connection survives a missed poll instead of flickering to connecting", () => {
  let mem = settleHostState(null, { kind: "connected" }, 0);
  mem = settleHostState(mem, { kind: "connecting" }, 3_000);
  assert.equal(mem.shown.kind, "connected");
  mem = settleHostState(mem, { kind: "connected" }, 6_000);
  assert.equal(mem.worseSince, null);
  mem = settleHostState(mem, { kind: "connecting" }, 7_000);
  mem = settleHostState(mem, { kind: "connecting" }, 7_000 + HOST_STATE_HOLD_MS);
  assert.equal(mem.shown.kind, "offline");
  mem = settleHostState(mem, { kind: "connecting" }, 30_000);
  assert.equal(mem.shown.kind, "offline");
  mem = settleHostState(mem, { kind: "connected" }, 31_000);
  assert.equal(mem.shown.kind, "connected");
});
