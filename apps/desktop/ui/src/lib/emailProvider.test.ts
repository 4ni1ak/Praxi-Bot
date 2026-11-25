import assert from "node:assert/strict";
import { test } from "node:test";
import { emailProviderKind, suggestInboxName } from "./emailProvider.ts";

test("every coarse provider sentence of the daemon becomes its own kind", () => {
  const prefix = "mailbox provider: ";
  for (const [message, kind] of [
    ["the provider rejected the API key", "auth"], // 401
    ["the API key is not allowed to do this", "forbidden"], // 403
    ["a limit of the provider plan was reached", "quota"], // 403 / 409 with a limit code
    ["rate limited by the provider, retry after 7s", "rateLimited"], // 429
    ["the provider is temporarily unavailable", "transient"], // 5xx, timeout
    ["the provider rejected the request (already_exists)", "taken"], // 409 name taken
    ["the provider rejected the request (conflict)", "taken"], // 409 without a code
    ["the provider rejected the request (validation_error)", "rejected"], // 422
    ["the provider rejected the request", "rejected"],
    ["something nobody mapped", "other"]
  ] as const) {
    assert.equal(emailProviderKind(prefix + message), kind, message);
    assert.equal(emailProviderKind(message), kind, message);
  }
});

test("a taken inbox name gets a short free-looking alternative that keeps the wish", () => {
  assert.equal(suggestInboxName("praxi", () => 0), "praxi-0000");
  assert.match(suggestInboxName("Atlas Bot!", () => 0.5), /^atlas-bot-[a-z0-9]{4}$/);
  assert.match(suggestInboxName("", () => 0.9), /^praxi-[a-z0-9]{4}$/);
  assert.notEqual(suggestInboxName("praxi", () => 0.1), suggestInboxName("praxi", () => 0.7));
});

test("only the provider's name-taken refusal behind email_provider_error counts as a taken inbox", async () => {
  const { isTakenInboxError } = await import("./emailProvider.ts");
  const taken = { code: "email_provider_error", message: "mailbox provider: the provider rejected the request (already_exists)" };
  assert.equal(isTakenInboxError(taken), true);
  assert.equal(isTakenInboxError(new Error("the provider rejected the request (already_exists)")), false);
  assert.equal(isTakenInboxError({ ...taken, code: "conflict" }), false);
  assert.equal(isTakenInboxError({ code: "email_provider_error", message: "the provider rejected the API key" }), false);
  assert.equal(isTakenInboxError(null), false);
});
