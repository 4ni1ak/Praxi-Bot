import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import { createElement, type ComponentType } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";
import type { ApprovalCardProps } from "../components/ChatCards.tsx";

let server: ViteDevServer;
let ApprovalCard: ComponentType<ApprovalCardProps>;
let openLink: (url: string, dashboard?: boolean) => Promise<boolean>;
let native: { fail: boolean; calls: unknown[] };
before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../..", import.meta.url)), logLevel: "error",
    optimizeDeps: { noDiscovery: true, entries: [] }, server: { middlewareMode: true, watch: null, hmr:false }, ssr: { noExternal: ["ui", "@tauri-apps/api"] },
    plugins: [{name:"fake-native-opener", enforce:"pre", resolveId(source) { if(source === "@tauri-apps/api/core") return "\0fake-native-opener"; },
      load(id) { if(id === "\0fake-native-opener") return `export const state = { fail: false, calls: [] }; export async function invoke(name, args) { state.calls.push([name,args]); if(state.fail) throw Error("OS failed"); }`; }}]
  });
  await server.ssrLoadModule("/src/i18n/index.ts");
  ({ ApprovalCard } = await server.ssrLoadModule("/src/components/ChatCards.tsx"));
  ({ openComposioLink: openLink } = await server.ssrLoadModule("/src/lib/externalLink.ts"));
  const api = await server.ssrLoadModule("@tauri-apps/api/core"); native = api.state;
});
after(async () => { await server?.close(); });

test("real card renders full long arguments with explicit expansion and trusted Composio footer", () => {
  const tail = "RECIPIENT_AFTER_LONG_PREFIX@example.test";
  const body = "a".repeat(1800) + tail;
  const html = renderToStaticMarkup(createElement(ApprovalCard, {
    tool: { kind:"tool", key:"call", callId:"call", requestedMs:1, gated:true,name:"app:github/do_thing",input:{x:"clipped"},summary:"clipped at 300",status:"awaiting_approval",
      details: { kind:"app_action",app:"github",app_name:"GitHub",action_slug:"GITHUB_DO_THING",class:"send",via:"Composio",version:"20260901_00",arguments:{ body, recipient:tail } } },
    onDecide:async () => {}
  }));
  assert.match(html,/data-testid="approval-app-action"/);
  assert.ok(html.includes(body) && html.includes(tail));
  assert.match(html,/<details><summary>[^<]*[0-9]+[^<]*<\/summary>/);
  assert.match(html,/GitHub/); assert.match(html,/GITHUB_DO_THING/); assert.match(html,/20260901_00/);
  assert.match(html,/Composio/); assert.ok(!html.includes("clipped at 300"));
  assert.ok(!html.includes("Always allow"));
});

test("native opener reports OS failures and enforces the exact consent/dashboard hosts", async () => {
  native.fail = true;
  assert.equal(await openLink("https://connect.composio.dev/link/x"),false);
  native.fail = false;
  assert.equal(await openLink("https://connect.composio.dev/link/x"),true);
  assert.equal(await openLink("https://platform.composio.dev/",true),true);
  const n = native.calls.length;
  assert.equal(await openLink("https://unexpected.composio.dev/"),false);
  assert.equal(await openLink("https://platform.composio.dev/"),false);
  assert.equal(native.calls.length,n);
  assert.deepEqual(native.calls[1],["open_composio_link",{href:"https://connect.composio.dev/link/x",dashboard:false}]);
});
