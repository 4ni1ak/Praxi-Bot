import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import { createElement, type ComponentType } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";
import type { ApprovalCardProps } from "../components/ChatCards.tsx";

let server: ViteDevServer;
let ApprovalCard: ComponentType<ApprovalCardProps>;

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
});
after(async () => {
  await server?.close();
});

const details = (warnings: unknown[]) => ({
  kind: "mail_send",
  account: "work",
  from: "me@example.test",
  to: ["ann@example.test"],
  cc: [],
  bcc: [],
  subject: "Hello",
  body: "Offer\n\n-- \nreply UNSUBSCRIBE\n",
  opt_out_line: true,
  warnings,
  legal_notice: warnings.length ? "legal" : null,
  send_anyway: warnings.length > 0
});

const render = (d: unknown): string =>
  renderToStaticMarkup(
    createElement(ApprovalCard, {
      tool: {
        kind: "tool", key: "c1", callId: "c1", requestedMs: 1, gated: true, name: "email_send",
        input: { to: ["ann@example.test"], subject: "Hello", body: "Offer" },
        details: d, summary: "", status: "awaiting_approval"
      } as ApprovalCardProps["tool"],
      onDecide: async () => {}
    })
  );

test("a mail card with an opt-out warning says so, shows the legal note and offers 'Send anyway'", () => {
  const html = render(details([{ code: "opt_out_listed", text: "x", addresses: ["ann@example.test"], count: 0, limit: 0 }]));
  assert.ok(html.includes("approval-mail-warnings"), html);
  assert.ok(html.includes("ann@example.test"));
  assert.ok(html.includes("6563"), "the legal sentence is shown once");
  assert.ok(html.includes("Send anyway"), "the approve button is the explicit choice");
  assert.ok(html.includes("UNSUBSCRIBE"), "the opt-out line the system adds is visible");
});

test("a mail card without warnings keeps the plain Send button", () => {
  const html = render(details([]));
  assert.ok(!html.includes("approval-mail-warnings"));
  assert.ok(!html.includes("Send anyway"));
});
