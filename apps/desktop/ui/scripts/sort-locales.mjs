/* global console, process, URL */
// Sorts every locale JSON alphabetically (all nesting levels, code-point order).
// Keeping keys sorted means parallel branches that add strings land in different
// places of the file instead of all appending at the tail (merge conflicts).
// Usage: node scripts/sort-locales.mjs [--check]
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const localeDir = fileURLToPath(new URL("../src/i18n/locales/", import.meta.url));
const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
export const sortDeep = (v) =>
  v && typeof v === "object" && !Array.isArray(v)
    ? Object.fromEntries(
        Object.keys(v)
          .sort(cmp)
          .map((k) => [k, sortDeep(v[k])])
      )
    : v;

const check = process.argv.includes("--check");
let bad = 0;
for (const file of readdirSync(localeDir).filter((f) => f.endsWith(".json")).sort()) {
  const path = join(localeDir, file);
  const src = readFileSync(path, "utf8");
  const out = `${JSON.stringify(sortDeep(JSON.parse(src)), null, 2)}\n`;
  if (out === src) continue;
  if (check) {
    console.error(`${file} is not sorted/normalised: run \`pnpm --dir apps/desktop/ui sort:i18n\``);
    bad++;
  } else {
    writeFileSync(path, out);
    console.log(`sorted ${file}`);
  }
}
if (bad) process.exit(1);
