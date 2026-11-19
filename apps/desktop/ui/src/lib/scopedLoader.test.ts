import assert from "node:assert/strict";
import { test } from "node:test";
import { createScopedLoader } from "./scopedLoader.ts";

test("switching host -> local re-fetches from the local target and drops the host's late answer", async () => {
  let shown: string[] | null = null;
  const loader = createScopedLoader<string[]>((v) => {
    shown = v;
  });
  const calls: string[] = [];
  let releaseHost: (v: string[]) => void = () => {};
  const host = { name: "host" };
  const local = { name: "local" };
  const hostLoad = loader.load(host, () => {
    calls.push("host");
    return new Promise<string[]>((r) => {
      releaseHost = r;
    });
  });
  // The user goes back to this computer before the host answered.
  const localLoad = loader.load(local, async () => {
    calls.push("local");
    return ["gemini", "openrouter", "cli:claude"];
  });
  assert.equal(await localLoad, true);
  releaseHost(["only-one"]);
  assert.equal(await hostLoad, false);
  assert.deepEqual(calls, ["host", "local"]);
  assert.deepEqual(shown, ["gemini", "openrouter", "cli:claude"]);
  loader.switchTo(host);
  assert.equal(shown, null, "the previous computer's list is cleared on switch");
});
