import type { ComputerApprovalDetails, EgressApprovalDetails } from "./approvalDetails.ts";

/** `t` as i18next gives it; a plain function keeps this module free of React. */
export type T = (key: string, values?: Record<string, unknown>) => string;

const KINDS = ["file", "window", "page", "mailbox", "mcp", "tool"] as const;

/** One source label the daemon recorded ("window msedge.exe") as a short phrase in the UI language. */
export function sourceText(label: string, t: T): string {
  const fixed: Record<string, string> = {
    "command output": "command",
    "workflow replay": "workflow",
    memory: "memory",
    "untrusted skill": "skill"
  };
  const key = fixed[label];
  if (key) return t(`chat.approval.plain.source.${key}`);
  const m = /^(\S+) (.+)$/s.exec(label);
  if (m && (KINDS as readonly string[]).includes(m[1])) {
    return t(`chat.approval.plain.source.${m[1]}`, { name: m[2] });
  }
  return label;
}

/** Why it asks, in one sentence, from the daemon's reason code. `null` for an older daemon (use its text). */
export function reasonLine(egress: EgressApprovalDetails, t: T): string | null {
  if (egress.reasonCode !== "egress" && egress.reasonCode !== "egress_executes") return null;
  const list = egress.read.length
    ? egress.read.map((s) => sourceText(s, t)).join(", ")
    : t("chat.approval.plain.reason.unknownSources");
  const key = egress.reasonCode === "egress_executes" || egress.executes ? "egressExecutes" : "egress";
  return t(`chat.approval.plain.reason.${key}`, { list, host: egress.host });
}

export interface WhereInput {
  /** The computer the daemon bound to a desktop/browser call, when it did. */
  computer: ComputerApprovalDetails | null;
  /** The bots' home server name (empty when unnamed), or `null` when the home is this computer. */
  serverName: string | null;
}

/** Which computer the call acts on, as words. */
export function whereText(tool: string, w: WhereInput, t: T): string {
  const onComputer = tool.startsWith("desktop_") || tool.startsWith("browser_");
  if (onComputer && w.computer?.kind === "pc" && w.serverName !== null) {
    return t("chat.approval.plain.where.user", { name: w.computer.name });
  }
  if (w.serverName === null) return t("chat.approval.plain.where.thisComputer");
  return w.serverName
    ? t("chat.approval.plain.where.server", { name: w.serverName })
    : t("chat.approval.plain.where.serverUnnamed");
}

function clip(value: unknown, max: number): string {
  const text = typeof value === "string" ? value : "";
  const line = text.split(/\r?\n/)[0] ?? "";
  const chars = [...line];
  return chars.length > max ? `${chars.slice(0, max).join("")}…` : line;
}

/** What the bot wants to do in plain words, or `null` for a tool without a plain sentence. */
export function actionText(tool: string, input: unknown, w: WhereInput, t: T): string | null {
  const rec = input && typeof input === "object" ? (input as Record<string, unknown>) : {};
  const where = whereText(tool, w, t);
  if (tool === "shell_run") {
    return t("chat.approval.plain.action.shell", { where, command: clip(rec.command, 120) });
  }
  if (tool === "desktop_launch") {
    return t("chat.approval.plain.action.launch", { where, name: clip(rec.executable, 80) });
  }
  if (tool === "browser_open") {
    return t("chat.approval.plain.action.browser", { where, url: clip(rec.url, 120) });
  }
  if (tool.startsWith("desktop_")) return t("chat.approval.plain.action.screen", { where });
  return null;
}
