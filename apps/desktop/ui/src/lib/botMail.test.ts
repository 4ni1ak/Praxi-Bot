import assert from "node:assert/strict";
import test from "node:test";
import type { EmailAccount, EmailTestReport } from "../api/types.ts";
import { accountsForBot, mailToolsOn, mailToolsOps, nextStep } from "./botMail.ts";

const acct = (name: string, bot: string | null): EmailAccount =>
  ({ name, bot, provider: "imap_smtp" }) as EmailAccount;

test("a bot sees its own mailbox and the shared ones, never another bot's", () => {
  const all = [acct("mine", "AAA"), acct("shared", null), acct("theirs", "BBB")];
  assert.deepEqual(
    accountsForBot(all, "aaa").map((a) => a.name),
    ["mine", "shared"]
  );
});

test("mail tools are off for a profile whose allow list lacks email_*", () => {
  const general = { tools: { allow: ["fs_read", "web_*", "mcp:*"] } };
  assert.equal(mailToolsOn(general), false);
  assert.equal(mailToolsOn({}), true);
  assert.equal(mailToolsOn({ tools: { allow: ["email_*"] } }), true);
  assert.equal(mailToolsOn({ tools: { allow: ["fs_read"], exclude: ["email_*"] } }), false);
});

test("turning mail on appends only the glob and keeps the other filters", () => {
  const cfg = { tools: { allow: ["fs_read", "web_*"], exclude: ["email_send", "shell_*"] } };
  assert.deepEqual(mailToolsOps(cfg, true), [
    { op: "set", path: "/tools/exclude", value: ["shell_*"] },
    { op: "set", path: "/tools/allow", value: ["fs_read", "web_*", "email_*"] }
  ]);
  assert.deepEqual(mailToolsOps({ tools: { allow: ["email_*"] } }, true), []);
});

test("turning mail off never widens an allow list to every tool", () => {
  assert.deepEqual(mailToolsOps({ tools: { allow: ["fs_read", "email_*"] } }, false), [
    { op: "set", path: "/tools/allow", value: ["fs_read"] }
  ]);
  assert.deepEqual(mailToolsOps({ tools: { allow: ["email_*"] } }, false), [
    { op: "set", path: "/tools/exclude", value: ["email_*"] }
  ]);
  assert.deepEqual(mailToolsOps({}, false), [{ op: "set", path: "/tools/exclude", value: ["email_*"] }]);
});

test("a failed check names the next step", () => {
  const r = (imap: EmailTestReport["imap"], smtp: EmailTestReport["smtp"], api?: EmailTestReport["api"]) =>
    ({ imap, smtp, ok: imap === "ok" && smtp === "ok", api }) as EmailTestReport;
  assert.equal(nextStep(r("ok", "ok")), "ok");
  assert.equal(nextStep(r("auth_failed", "ok")), "authFailed");
  assert.equal(nextStep(r("auth_failed", "auth_failed", "auth_failed")), "apiKey");
  assert.equal(nextStep(r("ok", "tls")), "tls");
});
