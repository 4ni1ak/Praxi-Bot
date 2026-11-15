import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { test } from "node:test";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";

interface Capability {
  identifier: string;
  windows?: string[];
  webviews?: string[];
  local?: boolean;
  remote?: unknown;
  permissions: (string | { identifier: string })[];
}

const config = JSON.parse(readFileSync(new URL("../../../tauri.conf.json", import.meta.url), "utf8")) as {
  app: { security: { capabilities: string[] } };
};
const directory = new URL("../../../capabilities/", import.meta.url);
const capabilities = readdirSync(directory)
  .filter((name) => name.endsWith(".json"))
  .map((name) => JSON.parse(readFileSync(new URL(name, directory), "utf8")) as Capability)
  .filter((capability) => config.app.security.capabilities.includes(capability.identifier));
const identifiers = (capability: Capability) => capability.permissions.map(
  (permission) => typeof permission === "string" ? permission : permission.identifier
);

// These methods explicitly require individual grants in @tauri-apps/api/window;
// core:window:default only grants read operations and internal-toggle-maximize.
const actions: [string, (window: Window) => Promise<void>][] = [
  ["set-decorations", (window) => window.setDecorations(false)],
  ["minimize", (window) => window.minimize()],
  ["toggle-maximize", (window) => window.toggleMaximize()],
  ["close", (window) => window.close()]
];

for (const [action, perform] of actions) {
  test(`the main window can invoke the ${action} command used by its chrome`, async () => {
    const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
    Object.defineProperty(globalThis, "window", { value: {}, configurable: true });
    try {
      mockWindows("main");
      mockIPC((command, payload) => {
        assert.equal(command, `plugin:window|${action.replaceAll("-", "_")}`);
        assert.equal((payload as { label: string }).label, "main");
        assert.ok(capabilities.some((capability) =>
          capability.windows?.includes("main") &&
          identifiers(capability).includes(`core:window:allow-${action}`)
        ), `${command} is not enabled by the shipped capabilities`);
      });
      await perform(getCurrentWindow());
    } finally {
      clearMocks();
      if (previous) Object.defineProperty(globalThis, "window", previous);
      else Reflect.deleteProperty(globalThis, "window");
    }
  });
}

test("window chrome grants stay local to the main window", () => {
  const grants = new Set(actions.map(([action]) => `core:window:allow-${action}`));
  for (const capability of capabilities.filter((c) => identifiers(c).some((p) => grants.has(p)))) {
    assert.deepEqual(capability.windows, ["main"]);
    assert.equal(capability.webviews, undefined);
    assert.equal(capability.remote, undefined);
    assert.notEqual(capability.local, false);
  }
});

// ── Custom app commands (card 239) ───────────────────────────────────────────
// build.rs lists every #[tauri::command] in the app manifest, so each one needs
// an `allow-<command>` permission in a capability that names the calling window.

const desktopDir = new URL("../../../", import.meta.url);
const read = (relative: string, base: URL = desktopDir) => readFileSync(new URL(relative, base), "utf8");

const manifestCommands = [...read("src/command_names.rs").matchAll(/^\s+"([a-z_]+)",$/gm)].map((m) => m[1]);

const toCommand = (permission: string) => permission.replace(/^allow-/, "").replaceAll("-", "_");
const matchesLabel = (pattern: string, label: string) =>
  pattern.endsWith("*") ? label.startsWith(pattern.slice(0, -1)) : pattern === label;

function commandsFor(label: string): string[] {
  const allowed = new Set<string>();
  for (const capability of capabilities) {
    if (!capability.windows?.some((pattern) => matchesLabel(pattern, label))) continue;
    for (const id of identifiers(capability)) {
      if (id.startsWith("allow-")) allowed.add(toCommand(id));
    }
  }
  return [...allowed].sort();
}

const widgetCommands = [
  "get_control_state",
  "give_back_control",
  "pause_bot_run",
  "resume_bot_run",
  "stop_bot_run",
  "take_the_wheel"
];

test("the app manifest lists the commands the shell registers", () => {
  assert.ok(manifestCommands.length >= 26, "command_names.rs could not be parsed");
  assert.ok(manifestCommands.includes("finish_bot_run"));
  assert.ok(manifestCommands.includes("ack_stop"));
  assert.ok(manifestCommands.includes("stop_bot_run"));
});

test("per-window custom command table", () => {
  assert.deepEqual(commandsFor("main"), [...manifestCommands].sort());
  assert.deepEqual(commandsFor("widget"), widgetCommands);
  assert.deepEqual(commandsFor("overlay-0"), ["get_control_state"]);
  assert.deepEqual(commandsFor("overlay-7"), ["get_control_state"]);
  assert.deepEqual(commandsFor("unknown-window"), []);
});

test("widget and overlays cannot call the run, daemon, provider or OS commands", () => {
  for (const label of ["widget", "overlay-0"]) {
    const allowed = commandsFor(label);
    for (const command of [
      "start_bot_run", "finish_bot_run", "ack_stop", "begin_action", "providers_cli", "set_autostart",
      "daemon_connect", "daemon_restart", "simulate_bot_run", "set_stop_hotkey", "notify_show"
    ]) {
      assert.ok(!allowed.includes(command), `${label} must not be granted ${command}`);
    }
  }
});

test("custom command grants are local to named windows", () => {
  for (const capability of capabilities.filter((c) => identifiers(c).some((id) => id.startsWith("allow-")))) {
    assert.ok((capability.windows ?? []).length > 0, `${capability.identifier} names no window`);
    assert.equal(capability.webviews, undefined);
    assert.equal(capability.remote, undefined);
    assert.notEqual(capability.local, false);
  }
});

/** invoke() command names reachable from the wrappers in overlay/events.ts, by exported function. */
function commandsByWrapper(): Map<string, string> {
  const source = read("src/overlay/events.ts", new URL("ui/", desktopDir));
  const wrappers = new Map<string, string>();
  for (const part of source.split(/^export (?:async )?function /m).slice(1)) {
    const name = /^(\w+)/.exec(part)?.[1];
    const command = /invoke(?:<[^>]*>)?\(\s*"(\w+)"/.exec(part.split(/^}/m)[0] ?? "")?.[1];
    if (name && command) wrappers.set(name, command);
  }
  return wrappers;
}

function commandsUsedBy(file: string): string[] {
  const source = read(`src/overlay/${file}`, new URL("ui/", desktopDir));
  assert.ok(!/invoke(?:<[^>]*>)?\(/.test(source), `${file} must call commands through overlay/events.ts`);
  const imported = /import\s*\{([^}]*)\}\s*from\s*"\.\/events"/.exec(source)?.[1] ?? "";
  const wrappers = commandsByWrapper();
  const used = new Set<string>();
  for (const name of imported.split(",").map((n) => n.replace(/^\s*type\s+/, "").trim())) {
    const command = wrappers.get(name);
    if (command) used.add(command);
  }
  return [...used].sort();
}

test("the widget UI only calls commands its capability grants", () => {
  const used = commandsUsedBy("WidgetWindow.tsx");
  assert.deepEqual(used, widgetCommands);
  for (const command of used) assert.ok(commandsFor("widget").includes(command), command);
});

test("the overlay UI only calls commands its capability grants", () => {
  const used = commandsUsedBy("OverlayWindow.tsx");
  assert.deepEqual(used, ["get_control_state"]);
  for (const command of used) assert.ok(commandsFor("overlay-0").includes(command), command);
});

test("every command the main UI invokes is granted to the main window", () => {
  const sources = ["src/api/connection.ts", "src/lib/providers.ts", "src/lib/shell.ts", "src/overlay/events.ts"];
  const invoked = sources.flatMap((file) =>
    [...read(file, new URL("ui/", desktopDir)).matchAll(/invoke(?:<[^>]*>)?\(\s*"(\w+)"/g)].map((m) => m[1])
  );
  assert.ok(invoked.length >= 15);
  for (const command of invoked) assert.ok(commandsFor("main").includes(command), `main lacks ${command}`);
});

test("the CSP is a real policy that keeps scripts local and allows only what the app uses", () => {
  const security = (JSON.parse(read("tauri.conf.json")) as { app: { security: { csp: unknown; devCsp: unknown } } }).app.security;
  assert.equal(typeof security.csp, "string", "csp must not be null");
  const csp = security.csp as string;
  const directives = new Map(csp.split("; ").map((d) => {
    const [name, ...sources] = d.split(" ");
    return [name, sources] as const;
  }));
  assert.deepEqual(directives.get("default-src"), ["'self'"]);
  assert.deepEqual(directives.get("script-src"), ["'self'"]);
  assert.deepEqual(directives.get("img-src"), ["'self'", "blob:", "data:"]);
  assert.deepEqual(directives.get("connect-src"), [
    "'self'", "ipc:", "http://ipc.localhost", "http://127.0.0.1:*", "http://localhost:*"
  ]);
  assert.deepEqual(directives.get("object-src"), ["'none'"]);
  assert.deepEqual(directives.get("frame-src"), ["'none'"]);
  assert.ok(!csp.includes("unsafe-eval"));
  assert.ok(!/https?:\/\/(?!127\.0\.0\.1|localhost|ipc\.localhost)/.test(csp), "no remote origin");
  assert.ok(!/(^|\s)\*(\s|;|$)/.test(csp), "no bare wildcard source");
  assert.equal(typeof security.devCsp, "string");
});
