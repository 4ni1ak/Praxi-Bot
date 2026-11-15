import assert from "node:assert/strict";
import { test } from "node:test";
import type { ComposioApp, ComposioToolkit } from "../api/types.ts";
import {
  appendPage,
  isComposioLink,
  pendingSlugs,
  upsertApp,
  withoutConnected
} from "./composioApps.ts";

const app = (slug: string, state: ComposioApp["state"]): ComposioApp => ({
  slug,
  name: slug,
  state,
  enabled: true,
  native_first: false,
  bots: null,
  tools: 0
});
const kit = (slug: string): ComposioToolkit => ({
  slug,
  name: slug,
  description: "",
  categories: [],
  tools_count: null,
  native_first: false,
  state: "not_connected",
  enabled: false
});

test("only https links on composio.dev may be opened", () => {
  assert.equal(isComposioLink("https://connect.composio.dev/link/x"), true);
  assert.equal(isComposioLink("https://composio.dev/"), true);
  for (const bad of [
    "http://connect.composio.dev/x",
    "https://evilcomposio.dev/x",
    "https://composio.dev.evil.com/x",
    "https://u:p@connect.composio.dev/x",
    "https://connect.composio.dev:8443/x",
    "http://169.254.169.254/latest",
    "javascript:alert(1)",
    "not a url",
    ""
  ]) {
    assert.equal(isComposioLink(bad), false, bad);
  }
});

test("pending apps, upsert and catalog helpers", () => {
  const apps = [app("a", "active"), app("b", "pending")];
  assert.deepEqual(pendingSlugs(apps), ["b"]);
  assert.equal(upsertApp(apps, app("b", "active"))[1]?.state, "active");
  assert.equal(upsertApp(apps, app("c", "pending")).length, 3);
  assert.deepEqual(
    withoutConnected([kit("a"), kit("z")], apps).map((t) => t.slug),
    ["z"]
  );
  assert.deepEqual(
    appendPage([kit("a")], [kit("a"), kit("b")]).map((t) => t.slug),
    ["a", "b"]
  );
});
