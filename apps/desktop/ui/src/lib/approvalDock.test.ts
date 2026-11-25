import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import { createElement, type ComponentType } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";
import type { ApprovalCardProps } from "../components/ChatCards.tsx";
import type { DockPage } from "../components/ApprovalDock.tsx";

let server: ViteDevServer;
let ApprovalCard: ComponentType<ApprovalCardProps>;
let ApprovalDock: ComponentType<{ pages: readonly DockPage[] }>;

before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../..", import.meta.url)),
    logLevel: "error",
    optimizeDeps: { noDiscovery: true, entries: [] },
    server: { middlewareMode: true, watch: null, hmr: false },
    ssr: { noExternal: ["ui", "@tauri-apps/api"] }
  });
  await server.ssrLoadModule("/src/i18n/index.ts");
  ({ ApprovalCard } = await server.ssrLoadModule("/src/components/ChatCards.tsx"));
  ({ ApprovalDock } = await server.ssrLoadModule("/src/components/ApprovalDock.tsx"));
});
after(async () => {
  await server?.close();
});

const tool = (callId: string, name = "browser_navigate", input: unknown = { url: "https://webioo.com.tr" }) =>
  ({
    kind: "tool",
    key: callId,
    callId,
    requestedMs: 1,
    gated: true,
    name,
    input,
    summary: "",
    status: "awaiting_approval"
  }) as ApprovalCardProps["tool"];

const page = (callId: string, name?: string): DockPage => ({
  key: callId,
  render: (active) =>
    createElement(ApprovalCard, { tool: tool(callId, name), variant: "dock", keyboard: active, onDecide: async () => {} })
});

test("one approval is one docked card with the three answers, no counter", () => {
  const html = renderToStaticMarkup(createElement(ApprovalDock, { pages: [page("c1")] }));
  assert.ok(html.includes('data-testid="approval-dock"'), "the dock is there");
  assert.ok(html.includes("data-approval-dock"), "the keyboard rules find the dock");
  assert.equal((html.match(/data-testid="approval-card"/g) ?? []).length, 1, "exactly one card");
  assert.ok(!html.includes("approval-dock-counter"), "no counter for a single approval");
  assert.ok(html.includes("webioo.com.tr"), "says what it wants to do");
  assert.ok(html.includes("approval-details"), "details are there, collapsed");
  assert.ok(!/<details[^>]*\sopen/.test(html), "details start collapsed");
});

test("several approvals are a counter on one dock, one card shown at a time", () => {
  const html = renderToStaticMarkup(createElement(ApprovalDock, { pages: [page("c1"), page("c2"), page("c3")] }));
  assert.equal((html.match(/data-testid="approval-card"/g) ?? []).length, 1, "one card at a time");
  assert.ok(html.includes("approval-dock-counter"));
  assert.ok(html.includes("Approval 1 of 3"), html);
});

test("an empty dock renders nothing", () => {
  assert.equal(renderToStaticMarkup(createElement(ApprovalDock, { pages: [] })), "");
});

test("a card that must be read does not advertise Enter", () => {
  const read: DockPage = { ...page("c9", "email_send"), keys: "deny" };
  const html = renderToStaticMarkup(createElement(ApprovalDock, { pages: [read] }));
  assert.ok(!html.includes("Allow once</span>"), "no Enter hint");
  assert.ok(html.includes("Deny</span>"), "Esc hint stays");
});

test("the chat asks approvals only in the dock: no inline card, no second strip, a new message is not queued behind it", () => {
  const view = readFileSync(new URL("../views/ChatView.tsx", import.meta.url), "utf8");
  // A pending approval renders nothing in the message stream.
  assert.match(view, /if \(item\.status === "awaiting_approval"\) \{\s*return null;\s*\}/);
  assert.doesNotMatch(view, /status === "awaiting_approval" \? \(\s*<ApprovalCard/);
  // The strip that repeated it is not drawn while the dock asks.
  assert.match(view, /approvalDocked && barModel\?\.kind === "waiting"/);
  // The dock sits in the composer wrapper, directly above the box.
  // (It is always mounted so it can fade out after the last answer; it renders nothing without pages.)
  assert.match(view, /<div className=\{styles\.boxWrap\}[^>]*>\s*<ApprovalDock pages=\{dockPages\}/);
  // A message typed while an approval waits goes to the daemon (which skips the approval).
  assert.match(view, /active && !approvalWaits && !body\.startsWith\("\/"\)/);
});

test("the card shows how much the call can change: a calm chip to look, a red one and its own words to delete", () => {
  const one = (name: string, input: unknown) =>
    renderToStaticMarkup(
      createElement(ApprovalDock, {
        pages: [
          {
            key: "r1",
            render: (active) =>
              createElement(ApprovalCard, { tool: tool("r1", name, input), variant: "dock", keyboard: active, onDecide: async () => {} })
          }
        ]
      })
    );
  const site = one("browser_navigate", { url: "https://webioo.com.tr" });
  assert.match(site, /data-risk="read"/);
  assert.ok(site.includes(">Allow</button>") || site.includes(">Allow<"), "a read keeps the plain Allow");
  const del = one("fs_delete", { path: "./drafts/old" });
  assert.match(del, /data-risk="delete"/);
  assert.ok(del.includes("Allow delete"), "the delete card names what Allow does");
  assert.ok(del.includes("Deletes"), "and carries the tier chip");
});
