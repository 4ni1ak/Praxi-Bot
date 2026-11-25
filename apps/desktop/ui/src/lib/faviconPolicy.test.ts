import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { chipFaviconDomain, mayFetchFavicon } from "./faviconPolicy.ts";

test("a favicon is requested only for a site that is already allowed, never for a pending card", () => {
  // A pending approval card names a model-chosen host: no lookup, whatever the host looks like.
  assert.equal(mayFetchFavicon(undefined, "secret-abc123.attacker.com"), false);
  assert.equal(mayFetchFavicon(false, "secret-abc123.attacker.com"), false);
  // Allowed sites (Allowed-sites list, activity chips after Allow) may fetch.
  assert.equal(mayFetchFavicon(true, "webioo.com.tr"), true);
  assert.equal(mayFetchFavicon(true, "not a host"), false);
});

test("an activity chip asks for its icon only after the step finished, and only for the host of its own input", () => {
  assert.equal(chipFaviconDomain("done", "WebiOO.com.tr"), "webioo.com.tr");
  for (const status of ["requested", "awaiting_approval", "running", "denied", "expired", "error"]) {
    assert.equal(chipFaviconDomain(status, "secret-abc123.attacker.com"), null, status);
  }
  // Not a public host name (IP, port, userinfo, empty): never fetched, even for a finished step.
  for (const host of ["127.0.0.1", "evil.com:8080", "u@evil.com", "", null, undefined]) {
    assert.equal(chipFaviconDomain("done", host), null, String(host));
  }
});

test("the chain wires the chip through that policy and keeps the letter / globe as the fallback", () => {
  const source = readFileSync(new URL("../components/ThinkingChain.tsx", import.meta.url), "utf8");
  assert.match(source, /chipFaviconDomain\(status, host\)/);
  assert.match(source, /<SiteMark host=\{site\.text\} status=\{tool\.status\} \/>/);
  assert.match(source, /onError=\{onError\}/);
  assert.match(source, /icon\.kind === "letter"/);
});
