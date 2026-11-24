import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { EmailAccount } from "../api/types.ts";
import { identitiesFor, isSharedDomain, senderOf, switchReason, usableBy } from "./mailIdentity.ts";

const base = {
  provider: "imap_smtp",
  inbox_id: null,
  username: "",
  imap: { host: "i", port: 993 },
  smtp: { host: "s", port: 465, security: "implicit" },
  drafts_folder: "Drafts",
  allowed_senders: [],
  password_set: true,
  custom_ca: false,
  shared_with: [],
  default_for: [],
  send_mode: "ask"
} as const;
const acc = (over: Partial<EmailAccount>): EmailAccount =>
  ({ ...base, name: "a", address: "a@x.com", bot: null, ...over }) as EmailAccount;

describe("mail identities", () => {
  it("a shared account is usable by the listed bots only", () => {
    const a = acc({ bot: "B1", shared_with: ["B2"] });
    assert.equal(usableBy(a, "b2"), true);
    assert.equal(usableBy(a, "B3"), false);
    assert.equal(usableBy(acc({}), "B3"), true);
  });

  it("the sender is the named account, else the bot's default", () => {
    const work = acc({ name: "work", address: "me@work.com" });
    const own = acc({ name: "own", address: "bot@agentmail.to", provider: "agentmail", bot: "B1", default_for: ["B1"] });
    const all = [work, own];
    assert.equal(senderOf(all, "B1", { account: "work" })?.address, "me@work.com");
    assert.equal(senderOf(all, "B1", {})?.name, "own");
    assert.equal(identitiesFor(all, "B1")[0].name, "own");
    assert.equal(senderOf([work, acc({ name: "two" })], "B1", {}), null);
  });

  it("flags the shared AgentMail domain and words the switch for the model", () => {
    assert.equal(isSharedDomain({ provider: "agentmail", address: "x@agentmail.to" }), true);
    assert.equal(isSharedDomain({ provider: "agentmail", address: "x@mine.com" }), false);
    assert.match(switchReason({ name: "work", address: "me@work.com" }), /account "work"/);
  });
});
