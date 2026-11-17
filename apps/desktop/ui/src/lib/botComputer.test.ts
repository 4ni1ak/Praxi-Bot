import assert from "node:assert/strict";
import { test } from "node:test";
import {
  hasBothComputers,
  serverDesktopCommand,
  serverScreenIssue,
  shownServer
} from "./botComputer.ts";

const host = { kind: "host" as const, hostId: "k", name: "Oracle" };

test("the card shows the server while one is home, this computer otherwise, and offers both only then", () => {
  assert.equal(shownServer(host, false, "bot"), "Oracle");
  assert.equal(shownServer(host, false, "mine"), null);
  assert.equal(shownServer(host, true, "bot"), null, "working here for now");
  assert.equal(shownServer({ kind: "local" }, false, "bot"), null);
  assert.equal(hasBothComputers(host, false), true);
  assert.equal(hasBothComputers({ kind: "local" }, false), false);
});

test("a server screen that cannot be shown says why, and the fix command is exact", () => {
  assert.equal(serverScreenIssue(null), "connecting");
  assert.equal(serverScreenIssue({ kind: "offline" }), "offline");
  assert.equal(serverScreenIssue({ kind: "not_paired" }), "notPaired");
  assert.equal(serverScreenIssue({ kind: "connected" }), null);
  assert.equal(serverDesktopCommand("no_display", "oracle"), "bot host repair oracle");
  assert.equal(serverDesktopCommand("no_display", "my server"), 'bot host repair "my server"');
  assert.equal(serverDesktopCommand("wayland", "oracle"), null);
  assert.equal(serverDesktopCommand(null, "oracle"), null);
});
