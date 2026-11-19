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

/** A short value on one line; anything cut (a longer text, more lines) ends in "…". */
function clip(value: unknown, max: number): string {
  const text = typeof value === "string" ? value : "";
  const lines = text.split(/\r?\n/);
  const line = lines[0] ?? "";
  const chars = [...line];
  if (chars.length > max) return `${chars.slice(0, max).join("")}…`;
  return lines.length > 1 ? `${line} …` : line;
}

/**
 * A command shown whole: every line, joined with " ⏎ " so a second command on another line is
 * never hidden; only a very long one is cut, and then it ends in "…".
 */
function whole(value: unknown, max: number): string {
  const text = typeof value === "string" ? value : "";
  const joined = text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l.length > 0)
    .join(" ⏎ ");
  const chars = [...joined];
  return chars.length > max ? `${chars.slice(0, max).join("")}…` : joined;
}

/** A desktop key list (`["ctrl","s"]` or `"ctrl+s"`) as text. */
function keysText(value: unknown): string {
  if (Array.isArray(value)) return clip(value.filter((k) => typeof k === "string").join("+"), 40);
  return clip(value, 40);
}

/** The `pc_browser_*` / `pc_desktop_*` call as a specific sentence. */
function pcComputerText(tool: string, rec: Record<string, unknown>, t: T): string | null {
  const ref = clip(rec.ref, 16);
  switch (tool) {
    case "pc_browser_open":
      return t("chat.approval.plain.action.pcBrowserOpen", { url: clip(rec.url, 200) });
    case "pc_browser_click":
      return t("chat.approval.plain.action.pcBrowserClick", { ref });
    case "pc_browser_fill":
      return t("chat.approval.plain.action.pcBrowserFill", { ref, text: whole(rec.text, 200) });
    case "pc_browser_select":
      return t("chat.approval.plain.action.pcBrowserSelect", { ref, value: clip(rec.value, 80) });
    case "pc_browser_press":
      return t("chat.approval.plain.action.pcBrowserPress", { key: clip(rec.key, 40) });
    case "pc_browser_back":
      return t("chat.approval.plain.action.pcBrowserBack");
    case "pc_browser_tabs":
      return typeof rec.switch_to === "number"
        ? t("chat.approval.plain.action.pcBrowserTab", { n: rec.switch_to })
        : t("chat.approval.plain.action.pcBrowserLook");
    case "pc_browser_snapshot":
    case "pc_browser_screenshot":
    case "pc_browser_wait":
    case "pc_browser_scroll":
      return t("chat.approval.plain.action.pcBrowserLook");
    case "pc_desktop_launch":
      return t("chat.approval.plain.action.pcLaunch", { name: whole([rec.executable, ...(Array.isArray(rec.args) ? rec.args : [])].filter((a) => typeof a === "string").join(" "), 200) });
    case "pc_desktop_type":
      return t("chat.approval.plain.action.pcScreenType", { text: whole(rec.text, 200) });
    case "pc_desktop_set_value":
      return t("chat.approval.plain.action.pcScreenType", { text: whole(rec.value, 200) });
    case "pc_desktop_key":
      return t("chat.approval.plain.action.pcScreenKeys", { keys: keysText(rec.keys) });
    case "pc_desktop_click":
    case "pc_desktop_drag":
      return t("chat.approval.plain.action.pcScreenClick");
    case "pc_desktop_get_state":
    case "pc_desktop_screenshot":
    case "pc_desktop_list_apps":
      return t("chat.approval.plain.action.pcScreenLook");
    default:
      if (tool.startsWith("pc_browser_")) return t("chat.approval.plain.action.pcBrowser");
      if (tool.startsWith("pc_desktop_")) return t("chat.approval.plain.action.pcScreen");
      return null;
  }
}

/** What the bot wants to do in plain words, or `null` for a tool without a plain sentence. */
export function actionText(tool: string, input: unknown, w: WhereInput, t: T): string | null {
  const rec = input && typeof input === "object" ? (input as Record<string, unknown>) : {};
  const where = whereText(tool, w, t);
  // The `pc_*` family always acts on the USER'S computer, whatever the bot runs on.
  if (tool.startsWith("pc_")) {
    if (tool === "pc_shell_run") return t("chat.approval.plain.action.pcShell", { command: whole(rec.command, 400) });
    if (tool === "pc_fs_write") return t("chat.approval.plain.action.pcWrite", { path: clip(rec.path, 200) });
    if (tool === "pc_fs_read" || tool === "pc_fs_list") {
      return t("chat.approval.plain.action.pcRead", { path: clip(rec.path, 200) });
    }
    return pcComputerText(tool, rec, t);
  }
  if (tool === "shell_run") {
    return t("chat.approval.plain.action.shell", { where, command: whole(rec.command, 400) });
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
