import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import type { i18n as I18n } from "i18next";
import { createServer, type ViteDevServer } from "vite";

type Layer = typeof import("./humanError.ts");

let server: ViteDevServer;
let layer: Layer;
let i18n: I18n;

before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../..", import.meta.url)),
    logLevel: "error",
    optimizeDeps: { noDiscovery: true, entries: [] },
    server: { middlewareMode: true, watch: null }
  });
  ({ i18n } = await server.ssrLoadModule("/src/i18n/index.ts"));
  layer = (await server.ssrLoadModule("/src/lib/humanError.ts")) as Layer;
});

after(async () => {
  await server?.close();
});

/** Every `"snake_code"` the Rust crates put next to an HTTP status in an error body. */
function rustCodes(): Set<string> {
  const root = fileURLToPath(new URL("../../../../../crates", import.meta.url));
  const found = new Set<string>();
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      if (statSync(p).isDirectory()) {
        walk(p);
      } else if (name.endsWith(".rs")) {
        for (const m of readFileSync(p, "utf8").matchAll(/StatusCode::[A-Z_0-9]+\s*,\s*"([a-z][a-z_0-9]*)"/g)) {
          found.add(m[1]);
        }
      }
    }
  };
  walk(root);
  return found;
}

test("every daemon error code has a title and a hint in both languages", async () => {
  const known = new Set<string>(layer.DAEMON_ERROR_CODES);
  for (const code of rustCodes()) {
    assert.ok(known.has(code), `the daemon can emit "${code}" but the UI has no text for it`);
  }
  for (const lng of ["en", "tr"]) {
    await i18n.changeLanguage(lng);
    for (const code of layer.DAEMON_ERROR_CODES) {
      const h = layer.humanError({ status: 409, code, message: "raw daemon text" });
      assert.equal(h.code, code);
      assert.ok(h.title && !h.title.startsWith("errors."), `${lng}/${code} title`);
      assert.ok(h.hint && !h.hint.startsWith("errors."), `${lng}/${code} hint`);
      assert.ok(!h.title.includes("raw daemon text"));
    }
  }
});

test("unknown codes and raw strings fall back to a human sentence; raw text only in details", async () => {
  await i18n.changeLanguage("en");
  const unknown = layer.humanError({ status: 0, code: "brand_new_code", message: "kaboom: stack at foo.rs:12" });
  assert.equal(unknown.code, "generic");
  assert.ok(!unknown.title.includes("kaboom") && !unknown.hint.includes("kaboom"));
  assert.match(unknown.details, /kaboom/);

  const control = layer.humanToolError(
    "Error: execution error: The user is controlling the screen right now (observation paused), so this read was not performed"
  );
  assert.match(control.title, /could not look at the screen/i);
  const uv = layer.humanToolError("cannot start MCP server `sqlite`: `uvx` was not found on PATH. Install uv");
  assert.match(uv.title, /uvx is not installed/);

  await i18n.changeLanguage("tr");
  assert.doesNotMatch(layer.humanError({ status: 409, code: "version_conflict", message: "PUT requires expected_version" }).title, /expected_version/);
  await i18n.changeLanguage("en");
});
