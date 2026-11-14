/**
 * CSS contract of the v6 shell (card 233): a conservative reader for the few rules of
 * `Sidebar.module.css` / `tokens.css` / `global.css` / `theme.ts` that decide the shell's
 * geometry.  No parser dependency: the files are regular enough for a rule-level regex, and
 * every failure names the rule, the file and the expected value.
 */

export interface CssSources {
  sidebarCss: string;
  tokensCss: string;
  globalCss: string;
  themeTs: string;
}

export interface ShellModel {
  /** Outer padding of the floating row, px. */
  floatPad: number;
  /** Gap between neighbouring columns in the floating row, px. */
  floatGap: number;
  /** Negative margin a closed panel pulls its gap back with in float mode, px (positive number). */
  closedPanelPull: number;
  /** Panel border widths per mode while open, px. */
  borders: {
    plain: { left: number; right: number };
    float: { left: number; right: number };
  };
  /** Inner column width given the panel width and the border widths in force. */
  innerWidth: (panelW: number, left: number, right: number, float: boolean) => number;
}

export interface ContractReport {
  model: ShellModel;
  failures: string[];
}

const FILE = {
  sidebar: "packages/ui/src/components/Sidebar.module.css",
  tokens: "packages/ui/tokens.css",
  global: "apps/desktop/ui/src/global.css",
  theme: "packages/ui/src/theme.ts"
};

const stripComments = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, "");
const norm = (s: string) => s.replace(/\s+/g, " ").trim();

interface Rule {
  selector: string;
  decls: Map<string, string>;
}

export function parseRules(css: string): Rule[] {
  const rules: Rule[] = [];
  const text = stripComments(css);
  const re = /([^{}]+)\{([^{}]*)\}/g;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const decls = new Map<string, string>();
    for (const part of m[2].split(";")) {
      const i = part.indexOf(":");
      if (i > 0) decls.set(part.slice(0, i).trim(), norm(part.slice(i + 1)));
    }
    for (const selector of m[1].split(",")) rules.push({ selector: norm(selector), decls });
  }
  return rules;
}

const find = (rules: Rule[], selector: string): Rule | undefined =>
  rules.find((r) => r.selector === selector);

/** Custom properties declared on `:root` in tokens.css (the plain, unconditional block). */
function rootVars(tokensCss: string): Map<string, string> {
  const vars = new Map<string, string>();
  for (const r of parseRules(tokensCss)) {
    if (r.selector !== ":root") continue;
    for (const [k, v] of r.decls) if (k.startsWith("--")) vars.set(k, v);
  }
  return vars;
}

/** Evaluate `12px`, `var(--x)`, `calc(a * n)`, `calc(a - b)`; undefined when it is not a plain length. */
export function evalLength(expr: string, vars: Map<string, string>, local = new Map<string, string>()): number | undefined {
  let text = norm(expr);
  for (let guard = 0; guard < 8 && text.includes("var("); guard++) {
    text = text.replace(/var\(\s*(--[\w-]+)\s*(?:,\s*((?:[^()]|\([^)]*\))*))?\)/g, (_m, name: string, fallback?: string) =>
      local.get(name) ?? vars.get(name) ?? fallback ?? "NaN"
    );
  }
  text = text.replace(/calc\(/g, "(").replace(/px/g, "");
  if (!/^[\d\s.+\-*/()]+$/.test(text)) return undefined;
  let pos = 0;
  const peek = (): string | undefined => {
    while (text[pos] === " ") pos++;
    return text[pos];
  };
  const number = (): number => {
    const c = peek();
    if (c === "(") {
      pos++;
      const v = sum();
      peek();
      pos++;
      return v;
    }
    if (c === "-") {
      pos++;
      return -number();
    }
    const m = /^\d*\.?\d+/.exec(text.slice(pos));
    if (!m) throw new Error("number");
    pos += m[0].length;
    return Number(m[0]);
  };
  const product = (): number => {
    let v = number();
    for (let c = peek(); c === "*" || c === "/"; c = peek()) {
      pos++;
      v = c === "*" ? v * number() : v / number();
    }
    return v;
  };
  const sum = (): number => {
    let v = product();
    for (let c = peek(); c === "+" || c === "-"; c = peek()) {
      pos++;
      v = c === "+" ? v + product() : v - product();
    }
    return v;
  };
  try {
    const v = sum();
    return Number.isFinite(v) ? v : undefined;
  } catch {
    return undefined;
  }
}

const FLOAT = ':global(:root[data-float="on"])';

/**
 * Read the contract.  `expect` carries the single-source numbers the TypeScript side exports
 * (`FLOAT_PAD`, `FLOAT_GAP`); the returned `model` is what the geometry check evaluates.
 */
export function readShellContract(src: CssSources, expect: { floatPad: number; floatGap: number }): ContractReport {
  const failures: string[] = [];
  const fail = (file: string, rule: string, expected: string, got: string | undefined) =>
    failures.push(`${file} :: ${rule}: expected ${expected}, found ${got ?? "nothing"}`);
  const rules = parseRules(src.sidebarCss);
  const vars = rootVars(src.tokensCss);
  const len = (expr: string | undefined, local?: Map<string, string>) =>
    expr === undefined ? undefined : evalLength(expr, vars, local);

  // (a) float padding and gap resolve to the numbers the app uses.
  const row = find(rules, `${FLOAT} .foregroundContainer`);
  const pad = len(row?.decls.get("padding"));
  const gap = len(row?.decls.get("gap"));
  if (pad !== expect.floatPad) fail(FILE.sidebar, `${FLOAT} .foregroundContainer padding`, `${expect.floatPad}px (FLOAT_PAD)`, row?.decls.get("padding"));
  if (gap !== expect.floatGap) fail(FILE.sidebar, `${FLOAT} .foregroundContainer gap`, `${expect.floatGap}px (FLOAT_GAP)`, row?.decls.get("gap"));

  // (c) panel keeps its width and clips.
  const panel = find(rules, ".panel");
  if (panel?.decls.get("flex-shrink") !== "0") fail(FILE.sidebar, ".panel flex-shrink", "0", panel?.decls.get("flex-shrink"));
  if (panel?.decls.get("overflow") !== "hidden") fail(FILE.sidebar, ".panel overflow", "hidden", panel?.decls.get("overflow"));

  // (d) the chat column may shrink instead of overflowing.
  const main = find(rules, ".mainPane");
  const minWidth = len(main?.decls.get("min-width"));
  if (minWidth !== 0) fail(FILE.sidebar, ".mainPane min-width", "var(--space-0) / 0", main?.decls.get("min-width"));

  // (e) borders count inside the box.
  if (!/\*\s*,\s*\*::before\s*,\s*\*::after\s*\{[^}]*box-sizing:\s*border-box/.test(stripComments(src.globalCss))) {
    fail(FILE.global, "*, *::before, *::after box-sizing", "border-box", undefined);
  }

  // float flag of the theme engine: glass or a backdrop.
  if (!/root\.dataset\.float\s*=\s*backdropActive\s*\|\|\s*style\s*===\s*"glass"/.test(src.themeTs)) {
    fail(FILE.theme, "data-float", 'backdropActive || style === "glass"', undefined);
  }

  // Panel borders while open: plain = border-left, float = border on all sides.
  const open = find(rules, '.panel[data-open="true"]');
  const floatOpen = find(rules, `${FLOAT} .panel[data-open="true"]`);
  const border = (decl: string | undefined): number => {
    const w = decl ? /^(\S+|calc\([^)]*\))\s+solid\b/.exec(decl)?.[1] : undefined;
    return (w ? len(w) : undefined) ?? 0;
  };
  const plainLeft = border(open?.decls.get("border-left"));
  const floatBorder = border(floatOpen?.decls.get("border"));
  const edgeVar = (r: Rule | undefined) => {
    const v = r?.decls.get("--panel-edge");
    return v === undefined ? undefined : len(v);
  };
  const plainEdge = edgeVar(open);
  const floatEdge = edgeVar(floatOpen);

  const closedFloat = find(rules, `${FLOAT} .panel`);
  const pull = len(closedFloat?.decls.get("margin-left"));
  if (pull !== -expect.floatGap) fail(FILE.sidebar, `${FLOAT} .panel margin-left`, `-${expect.floatGap}px (-FLOAT_GAP)`, closedFloat?.decls.get("margin-left"));

  // (b) the inner column must be derived from the panel's content box, not its border box.
  const inner = find(rules, ".panelInner");
  const width = inner?.decls.get("width");
  if (width === undefined || /^var\(--panel-w\)$/.test(width)) {
    fail(FILE.sidebar, ".panelInner width", "100% or calc(var(--panel-w) - var(--panel-edge)), not var(--panel-w) (it is clipped by the panel's own border)", width);
  }
  const innerWidth: ShellModel["innerWidth"] = (panelW, left, right, isFloat) => {
    if (width === undefined) return panelW;
    if (width === "100%") return panelW - left - right;
    const edge = isFloat ? floatEdge : plainEdge;
    const local = new Map<string, string>([["--panel-w", `${panelW}px`]]);
    if (edge !== undefined) local.set("--panel-edge", `${edge}px`);
    return len(width, local) ?? panelW;
  };

  return {
    failures,
    model: {
      floatPad: pad ?? expect.floatPad,
      floatGap: gap ?? expect.floatGap,
      closedPanelPull: pull === undefined ? expect.floatGap : -pull,
      borders: {
        plain: { left: plainLeft, right: 0 },
        float: { left: floatBorder, right: floatBorder }
      },
      innerWidth
    }
  };
}
