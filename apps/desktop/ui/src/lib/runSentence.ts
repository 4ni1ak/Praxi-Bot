import type { TFunction } from "i18next";
import type { ToolItem } from "../state/timeline.ts";
import { brandIcon } from "./brandIcons.ts";
import { describeTool, stepPhase } from "./toolLabels.ts";

const brandName = (name: string): string | null => {
  const b = brandIcon(name);
  return b.known ? b.name : null;
};

/** One human sentence for a tool step ("Searching the web"), in the step's own phase. */
export function toolSentence(t: TFunction, tool: ToolItem): string {
  const label = describeTool(tool.name, tool.input, brandName);
  return t(`${label.key}.${stepPhase(tool.status)}`, label.values);
}
