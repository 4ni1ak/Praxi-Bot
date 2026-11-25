/**
 * The corner radius of every card, strip, bubble, input and button comes from the radius tokens
 * that Settings > Appearance > Corners drives (`--corner-scale`). A component that names a radius
 * in pixels, or that uses a `--radius-*` token nobody defines, ignores the setting: an undefined
 * token makes the corners sharp (the approval card and the strip lost their rounding that way).
 */
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const uiSrc = join(here, "..");
const packagesUi = join(here, "../../../../../packages/ui");

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name === "dist") continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path, out);
    else out.push(path);
  }
  return out;
}

const tokensCss = readFileSync(join(packagesUi, "tokens.css"), "utf8");
const cssFiles = [...walk(uiSrc), ...walk(join(packagesUi, "src"))].filter((f) => f.endsWith(".css"));
// Radius tokens live in tokens.css; the bubble radius is derived from them in the chat stylesheet.
const allCss = [tokensCss, ...cssFiles.map((f) => readFileSync(f, "utf8"))].join("\n");
const defined = new Set([...allCss.matchAll(/(--(?:radius|bubble)-[a-z0-9-]+)\s*:/g)].map((m) => m[1]!));

test("every radius token is defined and scales with the corner setting", () => {
  for (const name of ["--radius-sm", "--radius-md", "--radius-control", "--radius-card", "--radius-modal", "--radius-composer"]) {
    assert.ok(defined.has(name), `${name} is defined in tokens.css`);
  }
  const scaled = (name: string, seen = new Set<string>()): boolean => {
    if (seen.has(name)) return false;
    seen.add(name);
    const m = new RegExp(`${name}\\s*:\\s*([^;]+);`).exec(allCss);
    if (!m) return false;
    const value = m[1]!;
    if (value.includes("var(--corner-scale)")) return true;
    return [...value.matchAll(/var\((--[a-z0-9-]+)\)/g)].some((r) => scaled(r[1]!, seen));
  };
  for (const name of ["--radius-sm", "--radius-md", "--radius-control", "--radius-card", "--radius-modal", "--radius-composer", "--bubble-radius"]) {
    assert.ok(scaled(name), `${name} follows --corner-scale (Appearance > Corners)`);
  }
  assert.match(tokensCss, /:root\[data-corners="sharp"\]\s*\{\s*--corner-scale/);
  assert.match(tokensCss, /:root\[data-corners="round"\]\s*\{\s*--corner-scale/);
});

test("no stylesheet uses a radius token that is not defined", () => {
  const unknown: string[] = [];
  for (const file of cssFiles) {
    const css = readFileSync(file, "utf8");
    for (const m of css.matchAll(/var\((--(?:radius|bubble)-[a-z0-9-]+)/g)) {
      if (!defined.has(m[1]!)) unknown.push(`${file.slice(uiSrc.length)}: ${m[1]}`);
    }
  }
  assert.deepEqual(unknown, [], "an undefined radius token renders sharp corners");
});

test("no stylesheet or component hard-codes a radius length", () => {
  const hardCoded: string[] = [];
  const literalLength = /(^|[\s(*/+-])\d*\.?\d+(px|rem|em|pt|vw|vh)\b/;
  for (const file of cssFiles) {
    const css = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    for (const m of css.matchAll(/(border(?:-[a-z]+)*-radius)\s*:\s*([^;}]+)/g)) {
      const value = m[2]!.trim();
      if (literalLength.test(` ${value}`)) hardCoded.push(`${file.slice(uiSrc.length)}: ${m[1]}: ${value}`);
    }
  }
  const tsxFiles = walk(uiSrc).filter((f) => f.endsWith(".tsx") && !f.endsWith(".test.tsx"));
  for (const file of tsxFiles) {
    const src = readFileSync(file, "utf8");
    for (const m of src.matchAll(/borderRadius\s*:\s*([^,}\n]+)/g)) {
      if (/^["'`]?\s*\d/.test(m[1]!.trim())) hardCoded.push(`${file.slice(uiSrc.length)}: borderRadius: ${m[1]!.trim()}`);
    }
  }
  assert.deepEqual(hardCoded, [], "use var(--radius-*) so Appearance > Corners works everywhere");
});

test("the approval dock and the composer share the composer radius token", () => {
  const dock = readFileSync(join(uiSrc, "components/ApprovalDock.module.css"), "utf8");
  const chat = readFileSync(join(uiSrc, "views/ChatView.module.css"), "utf8");
  assert.match(dock, /border-radius:\s*var\(--radius-composer\)\s+var\(--radius-composer\)\s+0\s+0/);
  assert.match(chat, /\.box\s*\{[^}]*border-radius:\s*var\(--radius-composer\)/);
});
