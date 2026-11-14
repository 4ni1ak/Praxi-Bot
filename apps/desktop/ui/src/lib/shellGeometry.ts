/**
 * Pure geometry of the v6 shell (card 233): where the rail, the chat, the right panel and the
 * panel's inner column land for a window width, using the SAME clamp functions the app uses
 * (`shellLayout.ts`) and the border / padding numbers read from the CSS (`shellContract.ts`).
 * `checkMatrix` asserts the layout invariants over the whole width x rail x panel x style grid.
 */
import { FLOAT_GAP, FLOAT_PAD, fitRail, MAIN_MIN, panelMax, PANEL_MIN } from "./shellLayout.ts";
import type { ShellModel } from "./shellContract.ts";

export interface Box {
  left: number;
  right: number;
}

export interface ShellInput {
  vw: number;
  /** Stored rail width (before the fit against the window). */
  rail: number;
  /** Stored panel width (before the clamp against the window). */
  panel: number;
  panelOpen: boolean;
  float: boolean;
}

export interface ShellBoxes {
  content: Box;
  rail: Box;
  main: Box;
  panel: Box;
  panelInner: Box;
  /** Right edge of the panel's content box (border-box right minus its right border). */
  panelContentRight: number;
}

/** The clamp functions under test: the app's, or an older variant for the mutation self-test. */
export interface ShellFns {
  panelMax: (vw: number, rail: number, float: boolean) => number;
  fitRail: ((vw: number, rail: number, panelW: number, float: boolean, panelOpen: boolean) => number) | null;
}

export const APP_FNS: ShellFns = { panelMax, fitRail };

export function shellBoxes(input: ShellInput, model: ShellModel, fns: ShellFns = APP_FNS): ShellBoxes {
  const { vw, float, panelOpen } = input;
  const pad = float ? model.floatPad : 0;
  const gap = float ? model.floatGap : 0;
  const panelW = panelOpen ? Math.min(input.panel, fns.panelMax(vw, input.rail, float)) : 0;
  const railW = fns.fitRail ? fns.fitRail(vw, input.rail, panelW, float, panelOpen) : input.rail;
  const content = { left: pad, right: vw - pad };
  const rail = { left: content.left, right: content.left + railW };
  const mainLeft = rail.right + gap;
  // flex: main takes what is left; a closed float panel (width 0) pulls its gap back.
  const pull = float && !panelOpen ? model.closedPanelPull : 0;
  const mainRight = content.right - panelW - (panelOpen ? gap : gap - pull);
  const main = { left: mainLeft, right: mainRight };
  const panelLeft = mainRight + (panelOpen ? gap : gap - pull);
  const panel = { left: panelLeft, right: panelLeft + panelW };
  const b = panelOpen ? (float ? model.borders.float : model.borders.plain) : { left: 0, right: 0 };
  const innerW = panelOpen ? model.innerWidth(panelW, b.left, b.right, float) : panelW;
  const innerLeft = panel.left + b.left;
  return {
    content,
    rail,
    main,
    panel,
    panelInner: { left: innerLeft, right: innerLeft + innerW },
    panelContentRight: panel.right - b.right
  };
}

export const MATRIX = {
  widths: [760, 900, 1024, 1120, 1400, 1920, 2560],
  rails: [68, 200, 264, 440],
  corners: { sharp: 0.4, soft: 1, round: 1.6 } as Record<string, number>
};

/** Panel cases of the matrix: closed, the two common widths and the largest the window allows. */
export function panelCases(vw: number, rail: number, float: boolean, fns: ShellFns): Array<number | "closed"> {
  return ["closed", 260, 320, fns.panelMax(vw, rail, float)];
}

export interface MatrixResult {
  cells: number;
  violations: string[];
  /** Smallest chat width seen and the cell that produced it. */
  narrowestMain: { width: number; cell: string };
}

const EPS = 0.01;

export function checkMatrix(model: ShellModel, fns: ShellFns = APP_FNS): MatrixResult {
  const violations: string[] = [];
  let cells = 0;
  let narrowest = { width: Infinity, cell: "" };
  const add = (cell: string, msg: string) => violations.push(`${cell}: ${msg}`);
  for (const vw of MATRIX.widths) {
    for (const rail of MATRIX.rails) {
      for (const float of [false, true]) {
        for (const panelCase of panelCases(vw, rail, float, fns)) {
          for (const [corners, scale] of Object.entries(MATRIX.corners)) {
            cells++;
            const open = panelCase !== "closed";
            const cell = `vw=${vw} rail=${rail} panel=${panelCase} float=${float ? "on" : "off"} corners=${corners}`;
            const g = shellBoxes({ vw, rail, panel: open ? panelCase : PANEL_MIN, panelOpen: open, float }, model, fns);
            const pad = float ? model.floatPad : 0;
            const mainW = g.main.right - g.main.left;
            if (mainW < narrowest.width) narrowest = { width: mainW, cell };
            const boxes: Array<[string, Box]> = [["rail", g.rail], ["main", g.main]];
            if (open) boxes.push(["panel", g.panel], ["panelInner", g.panelInner]);
            for (const [name, b] of boxes) {
              if (b.left < pad - EPS || b.right > vw - pad + EPS) {
                add(cell, `${name} [${b.left}, ${b.right}] leaves [${pad}, ${vw - pad}]`);
              }
            }
            if (g.rail.right > g.main.left + EPS) add(cell, "rail overlaps main");
            if (open && g.main.right > g.panel.left + EPS) add(cell, "main overlaps panel");
            if (mainW < 280 - EPS) add(cell, `main ${mainW}px < 280`);
            if (vw >= 900 && mainW < MAIN_MIN - EPS) add(cell, `main ${mainW}px < MAIN_MIN ${MAIN_MIN}`);
            if (open) {
              if (Math.abs(g.panel.right - (vw - pad)) > EPS) add(cell, `panel.right ${g.panel.right} != ${vw - pad}`);
              if (g.panelInner.right > g.panelContentRight + EPS) {
                add(cell, `panelInner.right ${g.panelInner.right} > panel content right ${g.panelContentRight} (clipped ${g.panelInner.right - g.panelContentRight}px)`);
              }
              // Corners: the panel must be able to hold its own radius (12px card radius x 1.35 x scale).
              const radius = 12 * scale * 1.35;
              if (float && 2 * radius > g.panel.right - g.panel.left) add(cell, `radius ${radius}px does not fit the panel`);
            }
            // scrollWidth <= vw: nothing sits right of the window.
            const rightMost = Math.max(g.rail.right, g.main.right, open ? g.panel.right : 0, open ? g.panelInner.right : 0);
            if (rightMost > vw + EPS) add(cell, `content reaches ${rightMost} > window ${vw}`);
          }
        }
      }
    }
  }
  return { cells, violations, narrowestMain: narrowest };
}

export { FLOAT_GAP, FLOAT_PAD };

import { readShellContract, type CssSources } from "./shellContract.ts";

export interface LayoutReport extends MatrixResult {
  contractFailures: string[];
}

/** Contract rules + the whole matrix, for the sources and clamp functions given. */
export function evaluateShell(src: CssSources, fns: ShellFns = APP_FNS): LayoutReport {
  const { model, failures } = readShellContract(src, { floatPad: FLOAT_PAD, floatGap: FLOAT_GAP });
  return { ...checkMatrix(model, fns), contractFailures: failures };
}
