import assert from "node:assert/strict";
import { test } from "node:test";
import type { ProvidersDto } from "../api/types.ts";
import { engineRows, filterModelGroups, isCliDefault, modelGroups, modelHint, recommendedOptions } from "./modelPicker.ts";
import { foldText, matchesQuery } from "./searchText.ts";

const dto = {
  active: [],
  catalog: [],
  default_provider: "gemini",
  configured: [
    { id: "anthropic", active: true, custom: false, default_model: "claude-sonnet-5-5", enabled: true, has_key: true, is_default: false, models: ["claude-opus-5-5"] },
    { id: "gemini", active: true, custom: false, default_model: "flash", enabled: true, has_key: true, is_default: true, models: [] },
    { id: "off", active: false, custom: false, default_model: "x", enabled: false, has_key: false, is_default: false, models: ["x"] }
  ]
} as ProvidersDto;

test("modelGroups groups by provider, default first, skips disabled", () => {
  const g = modelGroups(dto);
  assert.deepEqual(
    g.map((x) => x.provider),
    ["gemini", "anthropic"]
  );
  assert.deepEqual(
    g[1].models.map((m) => m.ref),
    ["anthropic/claude-opus-5-5", "anthropic/claude-sonnet-5-5"]
  );
});

test("modelGroups keeps the current model even when unlisted", () => {
  const g = modelGroups(dto, "ollama/qwen3:8b");
  assert.ok(g.some((x) => x.provider === "ollama" && x.models[0].model === "qwen3:8b"));
  assert.deepEqual(modelGroups(null), []);
});

test("modelGroups wires daemon display names and recommendation order without changing ids", () => {
  const named = structuredClone(dto);
  named.configured[0].model_details = [
    { id: "claude-sonnet-5-5", display_name: "Claude Sonnet 5.5", kind: "chat", recommended: true, recommended_rank: 1, deprecated: false, note: null },
    { id: "claude-opus-5-5", display_name: "Claude Opus 5.5", kind: "chat", recommended: true, recommended_rank: 2, deprecated: false, note: null }
  ];
  const groups = modelGroups(named, "anthropic/custom-id");
  const models = groups.find((g) => g.provider === "anthropic")?.models;
  assert.deepEqual(models?.map((m) => m.ref), [
    "anthropic/claude-sonnet-5-5", "anthropic/claude-opus-5-5", "anthropic/custom-id"
  ]);
  assert.equal(models?.[0].displayName, "Claude Sonnet 5.5");
  assert.equal(models?.[2].displayName, "Custom Id");
  assert.equal(models?.[2].recommended, false);
  assert.equal(filterModelGroups(groups, "Sonnet 5.5")[0].models[0].ref, "anthropic/claude-sonnet-5-5");
});

test("filterModelGroups searches provider and model", () => {
  const g = modelGroups(dto);
  assert.deepEqual(
    filterModelGroups(g, "opus").map((x) => x.provider),
    ["anthropic"]
  );
  assert.equal(filterModelGroups(g, "gemini").length, 1);
  assert.deepEqual(filterModelGroups(g, "nomatch"), []);
});

test("metadata-aware filtering does not reinsert a retired or non-chat default", () => {
  const filtered = structuredClone(dto);
  filtered.configured[1].default_model = "gemini-2.0-flash";
  filtered.configured[1].models = ["gemini-4-carbon"];
  filtered.configured[1].model_details = [
    { id: "gemini-4-carbon", display_name: "Gemini 4 Carbon", kind: "chat", recommended: true, recommended_rank: 1, deprecated: false, note: null }
  ];
  const ids = modelGroups(filtered).find((g) => g.provider === "gemini")?.models.map((m) => m.model);
  assert.deepEqual(ids, ["gemini-4-carbon"]);
  // An existing bot's explicit current id remains editable and executable.
  const current = modelGroups(filtered, "gemini/gemini-2.0-flash")
    .find((g) => g.provider === "gemini")?.models.map((m) => m.model);
  assert.ok(current?.includes("gemini-2.0-flash"));
});

test("search folding handles Turkish letters and diacritics", () => {
  assert.equal(foldText("İstanbul ÇIKIŞ ığü"), "istanbul cikis igu");
  assert.ok(matchesQuery("baglan", "Bağlandı"));
  assert.ok(matchesQuery("tema renk", "Tema", "Vurgu renk"));
  assert.ok(!matchesQuery("tema xyz", "Tema"));
  assert.ok(matchesQuery("", "anything"));
});

test("modelGroups includes enabled CLI providers with single default model and excludeCli filters them out", () => {
  const withCli = {
    active: ["cli:claude", "gemini"],
    default_provider: "gemini",
    configured: [
      {
        id: "gemini",
        active: true,
        custom: false,
        default_model: "flash",
        enabled: true,
        has_key: true,
        is_default: true,
        models: []
      },
      {
        id: "cli:claude",
        active: true,
        custom: false,
        default_model: null,
        enabled: true,
        has_key: false,
        is_default: false,
        models: []
      }
    ],
    catalog: [
      {
        id: "cli:claude",
        kind: "cli",
        base_url: "",
        auth: "none",
        key_env: null,
        tools: true,
        vision: false,
        configured: true
      }
    ]
  } as unknown as ProvidersDto;

  const execGroups = modelGroups(withCli);
  const cliGroup = execGroups.find((g) => g.provider === "cli:claude");
  assert.ok(cliGroup, "cli:claude should be present in executor groups");
  assert.deepEqual(
    cliGroup.models.map((m) => m.ref),
    ["cli:claude/default"]
  );

  const classifierGroups = modelGroups(withCli, null, { includeCli: false });
  assert.equal(
    classifierGroups.some((g) => g.provider === "cli:claude"),
    false
  );
});

test("recommendedOptions: at most four, spread over providers, CLI default never recommended", () => {
  const rec = (id: string, rank: number) => ({ id, display_name: id, kind: "chat", recommended: true, recommended_rank: rank, deprecated: false, note: null });
  const d = structuredClone(dto);
  d.configured[0].model_details = [rec("claude-sonnet-5-5", 1), rec("claude-opus-5-5", 2), rec("claude-haiku-5-5", 3)] as never;
  d.configured[0].models = ["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5"];
  d.configured[1].model_details = [rec("gemini-3.5-flash-lite", 1), rec("gemini-4-carbon", 2)] as never;
  d.configured[1].models = ["gemini-3.5-flash-lite", "gemini-4-carbon"];
  d.configured.push({ id: "cli:claude", active: true, custom: false, default_model: null, enabled: true, has_key: false, is_default: false, models: ["default", "sonnet"],
    model_details: [rec("default", 1), rec("sonnet", 2)] } as never);
  const picked = recommendedOptions(modelGroups(d));
  assert.equal(picked.length, 4);
  assert.deepEqual(picked.map((m) => m.ref), ["gemini/gemini-3.5-flash-lite", "anthropic/claude-sonnet-5-5", "cli:claude/sonnet", "gemini/gemini-4-carbon"]);
  assert.ok(!picked.some(isCliDefault));
  assert.equal(modelHint("gemini-3.5-flash-lite"), "fast");
  assert.equal(modelHint("claude-opus-5-5"), "strongest");
});

test("engineRows: installed-not-enabled connect first, uninstalled install last, enabled hidden", () => {
  const e = (id: string, installed: boolean, install: boolean, runnable = true) =>
    ({ detected: { id, display_name: id, installed, runnable }, install: install ? { command: "npm" } : null }) as never;
  const rows = engineRows([e("opencode", false, true), e("claude", true, true), e("codex", true, true), e("chatgpt-plan", false, false), e("cursor", true, true, false)], new Set(["claude"]));
  assert.deepEqual(rows.map((r) => `${r.action}:${r.id}`), ["connect:codex", "install:opencode"]);
});

test("model names fall back to a prettified id and a CLI model shows as a name", () => {
  const g = modelGroups(dto, "cli:opencode/google/gemini-2.5-pro");
  const m = g.find((x) => x.provider === "cli:opencode")!.models[0];
  assert.notEqual(m.displayName, m.model);
});
