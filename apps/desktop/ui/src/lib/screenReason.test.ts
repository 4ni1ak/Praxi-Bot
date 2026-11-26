import assert from "node:assert/strict";
import { test } from "node:test";
import { DESKTOP_SESSION_COMMAND, explainScreen, thisOs } from "./screenReason.ts";

const windowsPc = { local: true, os: "windows" } as const;
const server = { local: false, os: "" } as const;

test("no display on a server is explained and comes with the exact session command", () => {
  const e = explainScreen("unavailable", "no_display", server);
  assert.equal(e?.detailKey, "liveView.noScreen.no_display");
  assert.equal(e?.fixKey, "liveView.noScreenFix.addSession");
  assert.equal(e?.command, DESKTOP_SESSION_COMMAND);
  assert.match(DESKTOP_SESSION_COMMAND, /install-linux\.sh --update --with-desktop-session$/);
});

test("a bare 'unavailable' still says what to check instead of repeating itself", () => {
  const e = explainScreen("unavailable", null);
  assert.equal(e?.fixKey, "liveView.noScreenFix.generic");
});

test("a specific reason is left to the viewer; other codes carry no command", () => {
  assert.equal(explainScreen("browser_idle", null), null);
  assert.equal(explainScreen("unavailable", "wayland")?.command, null);
});

test("the Linux install command is shown for a server only, never for this PC", () => {
  for (const [reason, code] of [["unavailable", null], ["unavailable", "no_display"], ["unavailable", "desktop_off"], ["timeout", null]] as const) {
    assert.equal(explainScreen(reason, code, windowsPc)?.command, null, `${reason}/${code} on a PC`);
  }
  assert.equal(explainScreen("unavailable", null, server)?.command, DESKTOP_SESSION_COMMAND);
});

test("a local Windows or macOS PC is told what to allow, and where the bot's browser is", () => {
  const win = explainScreen("unavailable", null, windowsPc);
  assert.equal(win?.fixKey, "liveView.noScreenFix.localWindows");
  assert.equal(win?.extraKey, "liveView.noScreenFix.browserOwnWindow");
  assert.equal(explainScreen("unavailable", "screen_unavailable", { local: true, os: "macos" })?.fixKey, "liveView.noScreenFix.localMac");
  assert.equal(explainScreen(null, "desktop_off", windowsPc)?.fixKey, "liveView.noScreenFix.localOff");
  assert.equal(explainScreen("unavailable", null, server)?.extraKey, null);
});

test("every spelling of 'the screen did not start' folds into one cause", () => {
  const a = explainScreen("timeout", null, windowsPc);
  const b = explainScreen(null, "timed_out", windowsPc);
  assert.equal(a?.detailKey, "liveView.noScreen.timed_out");
  assert.equal(b?.detailKey, a?.detailKey);
  assert.equal(explainScreen("timeout", null, server)?.fixKey, "liveView.noScreenFix.wait");
  assert.equal(explainScreen("timeout", null, server)?.command, null);
});

test("thisOs reads the viewer's system", () => {
  assert.equal(thisOs("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"), "windows");
  assert.equal(thisOs("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)"), "macos");
  assert.equal(thisOs("Mozilla/5.0 (X11; Linux x86_64)"), "linux");
});
