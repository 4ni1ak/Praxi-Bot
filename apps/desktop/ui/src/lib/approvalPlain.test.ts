import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import type { i18n as I18n } from "i18next";
import { createServer, type ViteDevServer } from "vite";
import { egressDetails } from "./approvalDetails.ts";

type Plain = typeof import("./approvalPlain.ts");

let server: ViteDevServer;
let plain: Plain;
let i18n: I18n;

before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../..", import.meta.url)),
    logLevel: "error",
    optimizeDeps: { noDiscovery: true, entries: [] },
    server: { middlewareMode: true, watch: null }
  });
  ({ i18n } = await server.ssrLoadModule("/src/i18n/index.ts"));
  plain = (await server.ssrLoadModule("/src/lib/approvalPlain.ts")) as Plain;
});

after(async () => {
  await server?.close();
});

const card = {
  kind: "egress",
  reason:
    "this run read window msedge.exe, window chrome.exe, command output and now wants to send data to the shell This call also starts a program on this computer, so it asks every time, even when sending data after reads is allowed.",
  reason_code: "egress_executes",
  executes: true,
  read: ["window msedge.exe", "window chrome.exe", "command output"],
  host: "the shell",
  payload: "which chromium",
  payload_kind: "body"
};

test("the approval reason and action render as short Turkish sentences, never the raw English line", async () => {
  await i18n.changeLanguage("tr");
  const t = ((k: string, v?: Record<string, unknown>) => i18n.t(k, v)) as import("./approvalPlain.ts").T;
  const egress = egressDetails(card)!;
  const reason = plain.reasonLine(egress, t)!;
  assert.match(reason, /Bot şunu okudu: pencere msedge\.exe, pencere chrome\.exe, komut çıktısı/);
  assert.match(reason, /program başlatmak/);
  assert.doesNotMatch(reason, /this run read|file shell_run/);
  const action = plain.actionText(
    "shell_run",
    { command: "which chromium || which google-chrome" },
    { computer: null, serverName: "Praxi host" },
    t
  )!;
  assert.equal(action, "Praxi host sunucusu üzerinde komut çalıştır: which chromium || which google-chrome");
  await i18n.changeLanguage("en");
  assert.match(plain.actionText("shell_run", { command: "ls" }, { computer: null, serverName: "Praxi host" }, t)!, /^Run a command on the Praxi host server: ls/);
});

test("an older daemon without a reason code keeps its own text", async () => {
  const t = ((k: string) => k) as import("./approvalPlain.ts").T;
  assert.equal(plain.reasonLine(egressDetails({ ...card, reason_code: undefined })!, t), null);
});

test("the user's computer is named as a separate target only for a call the daemon bound to it", async () => {
  await i18n.changeLanguage("en");
  const t = ((k: string, v?: Record<string, unknown>) => i18n.t(k, v)) as import("./approvalPlain.ts").T;
  const pc = { kind: "pc", name: "Praxi desktop", nodeId: "abc" };
  assert.match(plain.actionText("desktop_launch", { executable: "notepad.exe" }, { computer: pc, serverName: "Praxi host" }, t)!, /on your computer \(Praxi desktop\)/);
  assert.match(plain.actionText("shell_run", { command: "ls" }, { computer: pc, serverName: "Praxi host" }, t)!, /the Praxi host server/);
});

test("every pc_* tool reads as a sentence about YOUR computer, in both languages", async () => {
  const w = { computer: null, serverName: "Praxi host" };
  for (const lang of ["en", "tr"]) {
    await i18n.changeLanguage(lang);
    const t = ((k: string, v?: Record<string, unknown>) => i18n.t(k, v)) as import("./approvalPlain.ts").T;
    const calls: [string, Record<string, unknown>][] = [
      ["pc_shell_run", { command: "dir" }],
      ["pc_fs_write", { path: "C:/a.txt" }],
      ["pc_fs_read", { path: "C:/a.txt" }],
      ["pc_fs_list", { path: "C:/" }],
      ["pc_browser_open", { url: "https://example.com" }],
      ["pc_browser_click", {}],
      ["pc_desktop_launch", { executable: "notepad.exe" }],
      ["pc_desktop_click", {}]
    ];
    for (const [tool, input] of calls) {
      const text = plain.actionText(tool, input, w, t)!;
      assert.ok(text && !text.includes("{{") && !text.startsWith("chat."), `${lang} ${tool}: ${text}`);
      assert.doesNotMatch(text, /Praxi host/, "never the server's name");
    }
  }
  await i18n.changeLanguage("en");
  const t = ((k: string, v?: Record<string, unknown>) => i18n.t(k, v)) as import("./approvalPlain.ts").T;
  assert.equal(plain.actionText("pc_shell_run", { command: "dir" }, w, t), "Run a command on YOUR computer: dir");
});
