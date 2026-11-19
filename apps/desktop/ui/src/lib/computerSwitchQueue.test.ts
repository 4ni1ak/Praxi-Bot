import assert from "node:assert/strict";
import { test } from "node:test";
import type { ComputerSwitches } from "../api/client.ts";
import { SwitchQueue } from "./computerSwitchQueue.ts";

const off: ComputerSwitches = { screen: false, alwaysRead: false, browser: false, commands: false, files: false };

test("two quick toggles never resend a stale switch set", async () => {
  const sent: ComputerSwitches[] = [];
  let release: () => void = () => {};
  const q = new SwitchQueue(
    (_id, next) => {
      sent.push(next);
      // The first request is still on its way when the second toggle happens.
      return sent.length === 1 ? new Promise<void>((r) => (release = r)) : Promise.resolve();
    },
    () => {}
  );
  // The page still shows the server's state (everything off) for both clicks.
  const first = q.change("h", off, { browser: true });
  assert.equal(q.view("h", off).browser, true, "the switch shows what was sent");
  const second = q.change("h", off, { commands: true });
  release();
  await Promise.all([first, second]);
  assert.deepEqual(sent, [
    { ...off, browser: true },
    { ...off, browser: true, commands: true }
  ]);
  // Revoking and quickly turning another on keeps the revocation.
  sent.length = 0;
  const server = { ...off, browser: true, commands: true };
  await Promise.all([q.change("h", server, { commands: false }), q.change("h", server, { files: true })]);
  assert.deepEqual(sent[1], { ...off, browser: true, files: true });
  assert.deepEqual(q.view("h", off), off, "nothing pending: the server's state shows");
});
