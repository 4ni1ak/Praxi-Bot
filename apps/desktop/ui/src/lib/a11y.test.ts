import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import ts from "typescript";

// Accessibility guards. jsx-a11y is not a dependency, so this scans the component sources for
// <button>/<a> elements that have no accessible name: no aria-label /
// aria-labelledby and no text (literal or `{...}` expression) among children.

const ROOTS = ["src/components", "src/views", "src/overlay"];
const NAMED_ATTRS = new Set(["aria-label", "aria-labelledby"]);

function walk(dir: string, out: string[]): void {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (p.endsWith(".tsx")) out.push(p);
  }
}

function hasName(attrs: ts.JsxAttributes): boolean {
  return attrs.properties.some(
    (p) =>
      ts.isJsxSpreadAttribute(p) ||
      (ts.isJsxAttribute(p) && NAMED_ATTRS.has(p.name.getText()))
  );
}

function hasText(children: readonly ts.JsxChild[]): boolean {
  return children.some((c) => {
    if (ts.isJsxText(c)) return /\S/.test(c.text);
    if (ts.isJsxExpression(c)) return c.expression !== undefined;
    if (ts.isJsxElement(c)) {
      const tag = c.openingElement.tagName.getText();
      // Plain HTML wrappers (<span>) carry text; capitalised ones are components.
      return /^[a-z]/.test(tag) ? hasText(c.children) : false;
    }
    return false;
  });
}

function findUnnamedButtons(file: string): string[] {
  const src = ts.createSourceFile(
    file,
    readFileSync(file, "utf8"),
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX
  );
  const bad: string[] = [];
  const visit = (n: ts.Node): void => {
    if (ts.isJsxElement(n)) {
      const tag = n.openingElement.tagName.getText();
      if (
        (tag === "button" || tag === "a") &&
        !hasName(n.openingElement.attributes) &&
        !hasText(n.children)
      ) {
        const { line } = src.getLineAndCharacterOfPosition(n.getStart());
        bad.push(`${file}:${line + 1}`);
      }
    }
    ts.forEachChild(n, visit);
  };
  visit(src);
  return bad;
}

test("every icon-only button or link has an aria-label", () => {
  const files: string[] = [];
  for (const r of ROOTS) walk(r, files);
  const bad = files.flatMap(findUnnamedButtons);
  assert.deepEqual(bad, []);
});

test("inline form errors are announced (role=alert)", () => {
  const files: string[] = [];
  for (const r of ROOTS) walk(r, files);
  const bad = files.flatMap((f) =>
    readFileSync(f, "utf8")
      .split("\n")
      .flatMap((line, i) =>
        /className=\{formStyles\.error\}/.test(line) && !/role=/.test(line) ? [`${f}:${i + 1}`] : []
      )
  );
  assert.deepEqual(bad, []);
});
