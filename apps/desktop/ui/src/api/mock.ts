import { applyPatchOps, type PatchOp } from "../lib/botPatch.ts";
import { ApiClient, ApiError } from "./client.ts";
import { MOCK_MCP_CATALOG } from "./mockCatalog.ts";
import { MockHostSetup } from "./mockHostSetup.ts";
import { MockLiveViewService } from "./mockLiveView.ts";
import type {
  Bot,
  BotSkills,
  Chat,
  ConfiguredProviderDto,
  ControlModeName,
  CreateSessionInput,
  DetectedProviderDto,
  DoctorCheck,
  EmailAccount,
  EmailAccountInput,
  InputBatch,
  ComposioApp,
  ComposioToolkit,
  McpServer,
  McpServerConfig,
  McpTool,
  Message,
  OwnSkill,
  PolicyDto,
  ProvidersDto,
  PutBotSkillsInput,
  ReleaseRequest,
  SessionUpdate,
  SkillAssignmentMode,
  SkillsCatalog,
  TakeoverRequest,
  UsageSummary,
  GlobalEnvelope,
  ViewHeartbeat,
  WebSearchConfig,
  WebSearchConfigInput,
  WebSearchTestInput
} from "./types.ts";
import type { KnowledgeSource, KnowledgeSourceInput, KnowledgeSettings, KnowledgeSettingsInput, KnowledgeSourceChange, KnowledgeEmbeddingApproval, KnowledgeEmbeddingGrantTarget } from "./types.ts";

import { isMockMode, MOCK_BASE_URL } from "./mockMode.ts";
export { isMockMode, MOCK_BASE_URL };

const NOW = 1_750_000_000_000;

const ATLAS = "11111111-1111-4111-8111-111111111111";
const NOVA = "22222222-2222-4222-8222-222222222222";

function bot(id: string, name: string, color: string, shape: string, model: string): Bot {
  return {
    id,
    name,
    created_at_ms: NOW,
    version: 1,
    config: {
      bot: { id: name.toLowerCase(), name, color, shape, persona: "" },
      models: { executor: model, planner: model, classifier: model }
    }
  };
}

const MOCK_SKILLS_CATALOG: SkillsCatalog = {
  skills: [
    {
      name: "code-review",
      description: "Inspect pull requests for subtle bugs, missing tests, and style regressions.",
      version: "1.2.0",
      body_sha256: "mock",
      scope: "bundled",
      requires: { apps: [], domains: [], keywords: ["review", "pr"] },
      hub: null
    },
    {
      name: "github-triage",
      description: "Categorize new issues, check duplicates, and suggest labels.",
      version: "1.0.4",
      body_sha256: "mock",
      scope: "shared",
      requires: { apps: ["github"], domains: ["github.com"], keywords: ["issues"] },
      hub: { source: "clawhub", locked: true }
    },
    {
      name: "meeting-notes",
      description: "Turn transcripts into crisp action items and owners.",
      version: "0.9.0",
      body_sha256: "mock",
      scope: "shared",
      requires: { apps: [], domains: [], keywords: [] },
      hub: null
    },
    {
      name: "release-notes",
      description: "Draft changelog highlights grouped by user impact.",
      version: "1.1.0",
      body_sha256: "mock",
      scope: "bundled",
      requires: { apps: [], domains: [], keywords: [] },
      hub: null
    }
  ],
  issues: []
};

const MOCK_OWN_SKILLS: Record<string, OwnSkill[]> = {
  [ATLAS]: [{ name: "atlas-standup", version: "0.1.0" }],
  [NOVA]: []
};

const SKILL_NAME_RE = /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/;

const BOTS: Bot[] = [
  bot(ATLAS, "Atlas", "#9D9AE8", "cloud", "anthropic/claude-sonnet-5-5"),
  bot(NOVA, "Nova", "#57BAA4", "diamond", "gemini/gemini-2.5-flash"),
  bot("bot-milo", "Milo", "#E4B75C", "cube", "openai/gpt-4.1"),
  bot("bot-ember", "Ember", "#DC8D76", "flower", "ollama/qwen3:8b")
];

const previews = ["Haftalık özetin hazır.", "Kaynakları inceliyorum…", "Tasarım dosyalarını düzenledim.", "Devam etmek için onayını bekliyorum."];
BOTS.forEach((b, i) => { Object.assign((b.config as { bot: object }).bot, { preview: previews[i], state: ["idle", "thinking", "working", "waiting-approval"][i], unread: i === 2 }); });
const LONG_PROMPT = Array.from({ length: 9 }, (_, i) => `${i + 1}. Geçen haftanın GitHub kapanan sorunlarını, takvimdeki toplantıları ve gelen kutusundaki yanıt bekleyen iletileri tara; her biri için tek cümlelik bir özet çıkar ve önem sırasına göre listele.`).join(String.fromCharCode(10));
const SCHEDULES = [
  { id: "sch-1", origin: "user", bot_id: ATLAS, name: "Haftalık özet", enabled: true, has_webhook: false, created_at_ms: NOW, last_run_at_ms: NOW - 6 * 86400000, next_run_at_ms: NOW + 86400000, misfire_policy: "skip", spec: { cron: "0 9 * * 1 Europe/Istanbul" }, payload: { kind: "agent_prompt", text: "Geçen haftanın GitHub ve takvim hareketlerinden kısa bir özet hazırla." } },
  { id: "sch-2", origin: "bot", bot_id: NOVA, name: "Kaynak taraması", enabled: true, has_webhook: true, created_at_ms: NOW, last_run_at_ms: null, next_run_at_ms: NOW + 3600000, misfire_policy: "run_once", spec: { every: { seconds: 3600 } }, payload: { kind: "agent_prompt", text: "Takip listesindeki sitelerde yeni içerik var mı bak." } },
  { id: "sch-3", origin: "user", bot_id: "bot-milo", name: "Yedek kontrolü", enabled: false, has_webhook: false, created_at_ms: NOW, last_run_at_ms: NOW - 86400000, next_run_at_ms: null, misfire_policy: "skip", spec: { cron: "30 22 * * * Europe/Istanbul" }, payload: { kind: "workflow", name: "backup-check", params: {} } }
] as unknown as import("./types").Schedule[];
const CHAT_ID = "33333333-3333-4333-8333-333333333333";
const CHATS: Chat[] = [
  {
    id: CHAT_ID,
    chat_id: CHAT_ID,
    bot_id: ATLAS,
    title: "Haftalık çalışma özeti",
    created_at_ms: NOW
  }
];

/** An aggregator with 400+ models (design review of the model picker): a few curated, mostly the long tail. */
const OPENROUTER_MODELS = (() => {
  const makers = ["meta-llama", "mistralai", "qwen", "deepseek", "cohere", "nousresearch", "perplexity", "x-ai", "amazon", "microsoft"];
  // Paid and free (`:free`) models; the picker ranks them from the data, nothing is flagged here.
  const out = [
    { ...mockModel("anthropic/claude-sonnet-5.5", "Claude Sonnet 5.5", null) },
    { ...mockModel("openai/gpt-5-mini", "GPT-5 Mini", null) },
    { ...mockModel("google/gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite", null) },
    { ...mockModel("deepseek/deepseek-chat", "DeepSeek Chat", null) },
    { ...mockModel("deepseek/deepseek-r1:free", "DeepSeek R1 (free)", null), note: "free" },
    { ...mockModel("meta-llama/llama-4-maverick:free", "Llama 4 Maverick (free)", null), note: "free" },
    { ...mockModel("qwen/qwen3-235b-a22b:free", "Qwen3 235B (free)", null), note: "free" },
    { ...mockModel("google/gemini-2.5-flash-lite:free", "Gemini 2.5 Flash-Lite (free)", null), note: "free" },
    { ...mockModel("mistralai/mistral-small-3.2:free", "Mistral Small 3.2 (free)", null), note: "free" }
  ];
  const tail = [
    { ...mockModel("openai/text-embedding-3-large", "Text Embedding 3 Large", null), kind: "embedding" },
    { ...mockModel("openai/gpt-4o-2024-08-06", "GPT-4o (2024-08-06)", null) },
    { ...mockModel("google/gemini-3.1-pro-preview", "Gemini 3.1 Pro (preview)", null), note: "preview" },
    { ...mockModel("openai/tts-1-hd", "TTS 1 HD", null), kind: "audio" }
  ];
  for (let i = 0; i < 406; i += 1) {
    const maker = makers[i % makers.length];
    out.push({ ...mockModel(`${maker}/model-${Math.floor(i / makers.length) + 1}b-${i % 3 === 0 ? "instruct" : "chat"}`, "", null) });
  }
  return [...out, ...tail].map((m) => ({ ...m, display_name: m.display_name || m.id.split("/")[1].replace(/-/g, " ") }));
})();
const PROVIDERS: ProvidersDto = {
  active: ["anthropic", "openai", "gemini", "ollama"],
  default_provider: "anthropic",
  catalog: [
    {
      id: "anthropic",
      auth: "api_key_env",
      base_url: "https://api.anthropic.com",
      configured: true,
      key_env: "ANTHROPIC_API_KEY",
      kind: "anthropic",
      tools: true,
      vision: true
    },
    {
      id: "gemini",
      auth: "api_key_env",
      base_url: "https://generativelanguage.googleapis.com",
      configured: true,
      key_env: "GEMINI_API_KEY",
      kind: "gemini",
      tools: true,
      vision: true
    },
    {
      id: "openrouter",
      auth: "api_key_env",
      base_url: "https://openrouter.ai/api/v1",
      configured: true,
      key_env: "OPENROUTER_API_KEY",
      kind: "openai_compatible",
      tools: true,
      vision: false
    },
    {
      id: "ollama",
      auth: "none",
      base_url: "http://localhost:11434/v1",
      configured: true,
      key_env: null,
      kind: "openai_compatible",
      tools: true,
      vision: false
    }
  ],
  configured: [
    { id: "openai", active: true, custom: false, default_model: "gpt-4.1", enabled: true, has_key: true, is_default: false, models: ["gpt-4.1", "o3"] },
    {
      id: "anthropic",
      active: true,
      custom: false,
      default_model: "claude-sonnet-5-5",
      enabled: true,
      has_key: true,
      is_default: true,
      models: ["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5", "claude-fable-5-2", "claude-fable-5-1", "claude-opus-5-1", "claude-sonnet-5-0", "claude-opus-4-1-20250805", "claude-3-5-haiku-20241022"],
      model_details: [
        mockModel("claude-sonnet-5-5", "Claude Sonnet 5.5", null),
        mockModel("claude-opus-5-5", "Claude Opus 5.5", null),
        mockModel("claude-haiku-5-5", "Claude Haiku 5.5", null),
        mockModel("claude-fable-5-2", "Claude Fable 5.2", null),
        mockModel("claude-fable-5-1", "Claude Fable 5.1", null),
        mockModel("claude-opus-5-1", "Claude Opus 5.1", null),
        mockModel("claude-sonnet-5-0", "Claude Sonnet 5", null),
        mockModel("claude-opus-4-1-20250805", "Claude Opus 4.1", null),
        mockModel("claude-3-5-haiku-20241022", "Claude Haiku 3.5", null)
      ]
    },
    {
      id: "gemini",
      active: true,
      custom: false,
      default_model: "gemini-2.5-flash",
      enabled: true,
      has_key: true,
      is_default: false,
      models: ["gemini-3.5-flash-lite", "gemini-2.5-flash", "gemini-2.5-flash-lite", "gemini-3.1-pro-preview", "gemini-2.5-pro", "gemini-3.5-flash", "antigravity-preview-05-2026", "deep-research-pro-preview"],
      model_details: [
        mockModel("gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite", null),
        mockModel("gemini-2.5-pro", "Gemini 2.5 Pro", null),
        mockModel("gemini-3.5-flash", "Gemini 3.5 Flash", null),
        mockModel("gemini-2.5-flash", "Gemini 2.5 Flash", null),
        mockModel("gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite", null),
        mockModel("gemini-3.1-pro-preview", "Gemini 3.1 Pro (preview)", null),
        mockModel("antigravity-preview-05-2026", "Antigravity Preview 05 2026", null),
        mockModel("deep-research-pro-preview", "Deep Research Pro Preview", null)
      ]
    },
    {
      id: "cli:claude",
      active: true,
      custom: false,
      default_model: null,
      enabled: true,
      has_key: false,
      is_default: false,
      models: ["default", "fable", "opus", "sonnet", "haiku"],
      model_details: [
        mockModel("default", "Claude Code default", null),
        mockModel("fable", "Claude Fable", null),
        mockModel("opus", "Claude Opus", null),
        mockModel("sonnet", "Claude Sonnet", null),
        mockModel("haiku", "Claude Haiku", null)
      ]
    },
    {
      id: "cli:opencode",
      active: true,
      custom: false,
      default_model: null,
      enabled: true,
      has_key: false,
      is_default: false,
      models: ["default", "opencode/big-pickle", "opencode/grok-code", "google/gemini-2.5-pro", "anthropic/claude-sonnet-4-5", "openai/gpt-5", "deepseek/deepseek-chat"],
      model_details: [
        mockModel("default", "OpenCode default", null),
        { ...mockModel("opencode/big-pickle", "Big Pickle (free)", null), note: "free" },
        { ...mockModel("opencode/grok-code", "Grok Code Fast (free)", null), note: "free" },
        mockModel("google/gemini-2.5-pro", "Gemini 2.5 Pro", null),
        mockModel("anthropic/claude-sonnet-4-5", "Claude Sonnet 4.5", null),
        mockModel("openai/gpt-5", "GPT-5", null),
        mockModel("deepseek/deepseek-chat", "DeepSeek Chat", null)
      ]
    },
    {
      id: "openrouter",
      active: true,
      custom: false,
      default_model: null,
      enabled: true,
      has_key: true,
      is_default: false,
      models: OPENROUTER_MODELS.map((m) => m.id),
      model_details: OPENROUTER_MODELS
    },
    {
      id: "ollama",
      active: true,
      custom: false,
      default_model: "qwen3:8b",
      enabled: true,
      has_key: false,
      is_default: false,
      models: ["qwen3:8b", "llama3.2", "qwen3:32b", "llama3.3:70b"]
    }
  ]
};

function mockModel(id: string, display_name: string, rank: number | null) {
  return { id, display_name, kind: "chat", recommended: rank !== null, recommended_rank: rank, deprecated: false, note: null };
}


const DETECTED_PROVIDERS: DetectedProviderDto[] = [
  {
    id: "claude",
    display_name: "Claude Code",
    installed: true,
    version: "2.1.42",
    auth: "signed_in",
    account_label: "emre@praxi.bot",
    runnable: true,
    detail: null,
    message: null
  },
  {
    id: "codex",
    display_name: "Codex",
    installed: true,
    version: "codex-cli 0.169.0",
    auth: "signed_in",
    account_label: "chatgpt@praxi.bot",
    runnable: false,
    detail: "codex_unconfinable",
    message:
      "The installed Codex cannot disable unified_exec, so Praxi will not run it."
  },
  {
    id: "cursor",
    display_name: "Cursor",
    installed: false,
    version: null,
    auth: "unknown",
    account_label: null,
    runnable: false,
    detail: "not_on_path",
    message: null
  },
  {
    id: "opencode",
    display_name: "OpenCode",
    installed: true,
    version: "opencode v2.3.0",
    auth: "unknown",
    account_label: null,
    runnable: false,
    detail: "auth_unverified",
    message: null
  },
  {
    id: "gemini",
    display_name: "Gemini CLI",
    installed: false,
    version: null,
    auth: "unknown",
    account_label: null,
    runnable: false,
    detail: "not_on_path",
    message: null
  },
  {
    id: "agy",
    display_name: "Antigravity",
    installed: true,
    version: "1.5.9",
    auth: "unknown",
    account_label: null,
    runnable: false,
    detail: "auth_unverified",
    message: null
  }
];

const MOCK_NPM: Record<string, string> = {
  claude: "@anthropic-ai/claude-code",
  codex: "@openai/codex",
  opencode: "opencode-ai",
  gemini: "@google/gemini-cli"
};

const USAGE: UsageSummary = {
  runs: 42,
  total: {
    model_calls: 311,
    prompt_tokens: 1_204_500,
    completion_tokens: 188_300,
    cached_tokens: 640_000,
    cost_usd: 3.4127
  },
  by_bot: [
    {
      bot_id: ATLAS,
      model_calls: 220,
      prompt_tokens: 900_000,
      completion_tokens: 140_000,
      cached_tokens: 500_000,
      cost_usd: 2.9
    },
    {
      bot_id: NOVA,
      model_calls: 91,
      prompt_tokens: 304_500,
      completion_tokens: 48_300,
      cached_tokens: 140_000,
      cost_usd: 0.5127
    }
  ]
};

const MCP_TOOLS: McpTool[] = [
  { name: "search", description: "Search", effect: "read" },
  { name: "create", description: "Create an item", effect: "write" }
];

function transportKind(
  transport: McpServerConfig["transport"]
): McpServer["transport"] {
  return typeof transport === "string" ? transport : transport.type;
}

function serverFromConfig(c: McpServerConfig, category = "other"): McpServer {
  return {
    name: c.name,
    transport: transportKind(c.transport),
    command: c.command ?? null,
    args: c.args ?? null,
    url: c.url ?? null,
    enabled: c.enabled ?? true,
    status: "connected",
    error: null,
    tool_count: 8,
    category: c.category ?? category,
    trust_hints: false,
    public_content: c.public_content ?? false,
    local_only: c.local_only ?? false,
    bots: c.bots ?? null
  };
}

/** Diagnostics of the mock daemon: one of each status, two fixable. */
const DOCTOR_CHECKS: DoctorCheck[] = [
  { id: "daemon", title: "daemon", status: "ok", detail: "running (v0.1.0-mock, API 1.0), up 3600 s", hint: null, repair: null },
  { id: "data_dir", title: "data dir", status: "ok", detail: "~/AppData/Roaming/praxi (writable)", hint: null, repair: null },
  { id: "unicode_path", title: "unicode paths", status: "ok", detail: "non-ASCII file names work in the data dir", hint: null, repair: null },
  { id: "providers", title: "providers", status: "fail", detail: "openai: API key not available; anthropic: configured", hint: "`bot secrets set provider.<id>.api_key` or `bot providers add <id>` to fix the key", repair: null },
  { id: "providers_loaded", title: "loaded providers", status: "warn", detail: "enabled but not loaded: anthropic", hint: "reload the providers; if that fails, see the providers check", repair: "reload_providers" },
  { id: "cli_accounts", title: "cli accounts", status: "warn", detail: "ready: Claude Code; signed out: Codex", hint: "open a terminal and sign in to the CLI once (e.g. `claude`, `codex login`)", repair: null },
  { id: "browser", title: "browser", status: "ok", detail: "C:/Program Files/Google/Chrome/Application/chrome.exe (found; not launched)", hint: null, repair: null },
  { id: "desktop_automation", title: "desktop automation", status: "ok", detail: "windows: accessibility tree, screenshots, app launch", hint: null, repair: null },
  { id: "peer_link", title: "peer link", status: "skipped", detail: "no peer or host link configured (optional)", hint: null, repair: null },
  { id: "disk_space", title: "disk space", status: "ok", detail: "48211 MiB free", hint: null, repair: null },
  { id: "clock", title: "clock", status: "ok", detail: "plausible (checked locally, not against network time)", hint: null, repair: null }
];

/**
 * An in-memory daemon: the typed client methods run unchanged against it, so
 * the UI and its tests exercise the real request paths.
 */
export class MockApiClient extends ApiClient {
  private readonly knowledgeSources = new Map<string, KnowledgeSource[]>();
  private readonly knowledgeSettings = new Map<string, KnowledgeSettings>();
  private readonly knowledgeApprovals = new Map<string, { bot: string; card: KnowledgeEmbeddingApproval; revision?: number }>();
  readonly servers: McpServer[];
  readonly providersDto: ProvidersDto = structuredClone(PROVIDERS);
  readonly detectedProvidersList: DetectedProviderDto[] = structuredClone(DETECTED_PROVIDERS);
  readonly bots = structuredClone(BOTS);
  readonly chats: Chat[] = structuredClone([...CHATS, ...BOTS.slice(1).map((b,i) => ({ id: `mock-chat-${i}`, chat_id: `mock-chat-${i}`, bot_id: b.id, title: previews[i+1], created_at_ms: NOW }))]);
  readonly messages = new Map<string, Message[]>();
  readonly secrets = new Map<string, number>();
  readonly doctorChecks: DoctorCheck[] = structuredClone(DOCTOR_CHECKS);
  /** Repairs run so far (tests). */
  readonly repairs: string[] = [];
  private readonly liveView = new MockLiveViewService();
  private readonly hostSetup = new MockHostSetup();
  private webSearchConfig: WebSearchConfig = {
    order: ["searxng", "brave", "tavily", "duckduckgo", "duckduckgo_lite", "bing"],
    active_backends: ["duckduckgo", "duckduckgo_lite", "bing"],
    searxng_url: null,
    brave_key_set: false,
    tavily_key_set: false,
    duckduckgo: true,
    duckduckgo_lite: true,
    bing: true
  };
  readonly emailAccounts: EmailAccount[] = [
    {
      name: "personal",
      provider: "imap_smtp",
      inbox_id: null,
      address: "alex@gmail.com",
      username: "",
      imap: { host: "imap.gmail.com", port: 993 },
      smtp: { host: "smtp.gmail.com", port: 465, security: "implicit" },
      drafts_folder: "Drafts",
      bot: null,
      allowed_senders: [],
      password_set: true,
      custom_ca: false
    },
    {
      name: "atlas-inbox",
      provider: "imap_smtp",
      inbox_id: null,
      address: "atlas@fastmail.example",
      username: "atlas",
      imap: { host: "imap.fastmail.example", port: 993 },
      smtp: { host: "smtp.fastmail.example", port: 587, security: "start_tls" },
      drafts_folder: "Drafts",
      bot: ATLAS,
      allowed_senders: ["alex@gmail.com"],
      password_set: true,
      custom_ca: false
    }
  ];

  private botSkills = new Map<
    string,
    { mode: SkillAssignmentMode; names: string[]; version: number }
  >([[NOVA, { mode: "selected", names: ["meeting-notes", "removed-skill"], version: 1 }]]);
  private botSkillsVersion = 1;

  constructor() {
    super({ base_url: MOCK_BASE_URL, token: "mock" });
    this.servers = [
      {
        ...serverFromConfig(
          {
            name: "github",
            transport: "stdio",
            command: "npx",
            args: ["-y", "@modelcontextprotocol/server-github"]
          },
          "developer"
        ),
        tool_count: 26,
        bots: null
      },
      {
        ...serverFromConfig(
          {
            name: "filesystem",
            transport: "stdio",
            command: "npx",
            args: ["-y", "@modelcontextprotocol/server-filesystem", "."]
          },
          "files"
        ),
        tool_count: 11,
        bots: [ATLAS]
      },
      serverFromConfig({ name: "gmail", transport: "http", url: "https://example.com/gmail" }, "communication"),
      serverFromConfig({ name: "google-calendar", transport: "http", url: "https://example.com/calendar", bots: [ATLAS, NOVA] }, "productivity"),
      serverFromConfig({ name: "slack", transport: "http", url: "https://example.com/slack", bots: [NOVA, "bot-milo"] }, "communication"),
      {
        ...serverFromConfig(
          { name: "linear", transport: "http", url: "https://mcp.linear.app/mcp" },
          "productivity"
        ),
        status: "error",
        error: "401 unauthorized: check the API key",
        tool_count: 0,
        bots: null
      }
    ];
  }

  protected override async requestText(method: string, path: string): Promise<string> {
    await Promise.resolve();
    if (method === "GET" && path === "/doctor/report") {
      const rows = this.doctorChecks
        .map((c) => `| ${c.status} | ${c.title} | ${c.detail} |`)
        .join("\n");
      return [
        "# Praxi diagnostics report",
        "",
        "- Version: 0.1.0-mock",
        "- Platform: windows/x86_64",
        "- Data dir: `~/AppData/Roaming/praxi`",
        "- Config files: providers.toml",
        "",
        "## Checks",
        "",
        "| Status | Check | Detail |",
        "| --- | --- | --- |",
        rows,
        ""
      ].join("\n");
    }
    throw new ApiError(404, "not_found", `mock: no text route ${method} ${path}`);
  }

  override async openViewFrameStream(
    sessionId: string,
    signal?: AbortSignal
  ): Promise<Response> {
    return this.liveView.openStream(sessionId, signal);
  }

  protected override async request<T>(
    method: string,
    path: string,
    opts: { body?: unknown } = {}
  ): Promise<T> {
    await Promise.resolve();
    const ok = (v: unknown) => v as T;
    const seg = path.split("/").filter(Boolean).map(decodeURIComponent);
    if (seg[0] === "hosts" && seg[1] === "setup") {
      const body = opts.body as { host: string; accept: boolean };
      if (!seg[2] && method === "POST") return ok(this.hostSetup.start(opts.body as never));
      if (seg[2] === "ssh-keys") return ok({ keys: this.hostSetup.keys() });
      if (seg[3] === "host-key") return ok(this.hostSetup.answerKey(seg[2], body.accept));
      if (seg[3] === "cancel") return ok(this.hostSetup.cancel(seg[2]));
      return ok(this.hostSetup.get(seg[2]));
    }
    if (seg[0] === "bots" && seg[2] === "knowledge") {
      if (!this.bots.some((bot) => bot.id === seg[1])) throw new ApiError(404, "not_found", "bot");
      const bot = seg[1];
      const sources = this.knowledgeSources.get(bot) ?? [];
      const settings: KnowledgeSettings = this.knowledgeSettings.get(bot) ?? { auto_retrieve: false, auto_retrieval_available: false, embedding: null, local_embeddings_available: false, local_profiles: [{ id: "offline-e5", label: "Installed multilingual model", model: "multilingual-e5-small", dimensions: 384 }] };
      if (seg[3] === "email-captures" && method === "GET") return ok([]);
      if (seg[3] === "settings") {
        if (method === "GET") return ok(settings);
        const input = opts.body as KnowledgeSettingsInput;
        const embedding = input.embedding;
        const identity = embedding ? JSON.stringify([embedding.provider_id, embedding.model, embedding.dimensions, embedding.request_dimensions, embedding.local_profile_id]) : "disabled";
        const fingerprint = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(identity)))).map((byte) => byte.toString(16).padStart(2, "0")).join("");
        const changed = settings.embedding?.profile_fingerprint !== fingerprint;
        if (changed) for (const source of sources) source.remote_profile_fingerprint = null;
        const next: KnowledgeSettings = { ...settings, auto_retrieve: false, embedding: embedding ? {
          provider_id: embedding.provider_id, model: embedding.model, dimensions: embedding.dimensions,
          request_dimensions: embedding.request_dimensions ?? false,
          endpoint: "https://embedding.example.invalid", profile_fingerprint: fingerprint,
          query_remote: !changed && (settings.embedding?.query_remote ?? false),
          requests_per_minute: embedding.requests_per_minute, requests_per_day: embedding.requests_per_day,
          local_profile_id: embedding.local_profile_id ?? null
        } : null };
        this.knowledgeSettings.set(bot, next);
        return ok(next);
      }
      if (seg[3] === "embedding-approvals") {
        const embedding = settings.embedding;
        if (!embedding || embedding.provider_id === "local-onnx") throw new ApiError(409, "conflict", "choose a remote embedding provider first");
        if (!seg[4]) {
          const target = opts.body as KnowledgeEmbeddingGrantTarget;
          const source = target.kind === "documents" ? sources.find((source) => source.id === target.source_id) : undefined;
          if (target.kind === "documents" && !source?.enabled) throw new ApiError(409, "conflict", "source must be enabled");
          const card: KnowledgeEmbeddingApproval = { id: crypto.randomUUID(), target, source_label: source?.label ?? null, profile_fingerprint: embedding.profile_fingerprint, provider_id: embedding.provider_id, model: embedding.model, endpoint_identity: embedding.endpoint, expires_at_ms: Date.now() + 300_000, policy_decision: "ask" };
          for (const [id, saved] of this.knowledgeApprovals) if (saved.bot === bot) this.knowledgeApprovals.delete(id);
          this.knowledgeApprovals.set(card.id, { bot, card, revision: source?.revision });
          return ok(card);
        }
        const saved = this.knowledgeApprovals.get(seg[4]);
        if (!saved || saved.bot !== bot || saved.card.expires_at_ms <= Date.now() || saved.card.profile_fingerprint !== embedding.profile_fingerprint) throw new ApiError(409, "conflict", "approval expired or changed");
        if (saved.card.target.kind === "documents") {
          const id = saved.card.target.source_id;
          const source = sources.find((source) => source.id === id);
          if (!source?.enabled || source.revision !== saved.revision) throw new ApiError(409, "conflict", "source changed");
          source.remote_profile_fingerprint = embedding.profile_fingerprint; source.revision += 1;
        } else embedding.query_remote = true;
        this.knowledgeApprovals.delete(seg[4]);
        return ok(undefined);
      }
      if (seg[3] === "embedding-grants") {
        const target = opts.body as KnowledgeEmbeddingGrantTarget;
        if (target.kind === "queries" && settings.embedding) settings.embedding.query_remote = false;
        if (target.kind === "documents") {
          const source = sources.find((source) => source.id === target.source_id);
          if (source?.remote_profile_fingerprint) { source.remote_profile_fingerprint = null; source.revision += 1; }
        }
        return ok(undefined);
      }
      if (seg[3] === "embedding-cooldown") return ok(undefined);
      if (seg[3] === "sources") {
        if (method === "GET") return ok(structuredClone(sources));
        if (method === "POST" && !seg[4]) {
          const input = opts.body as KnowledgeSourceInput;
          const source: KnowledgeSource = { id: crypto.randomUUID(), kind: input.kind, label: input.label ?? "Shared brain",
            locator: input.path ?? input.url ?? `saved:${input.kind}`, enabled: true, requires_path_selection: false, revision: 1,
            remote_profile_fingerprint: null, documents: 1, chunks: 1, updated_ms: Date.now() };
          sources.push(source); this.knowledgeSources.set(bot, sources);
          return ok({ source, indexing: { processed: 1, unchanged: 0, removed: 0, next_cursor: null, failures: [] } });
        }
        const source = sources.find((item) => item.id === seg[4]);
        if (!source) throw new ApiError(404, "not_found", "source");
        const input = opts.body as KnowledgeSourceChange;
        if (source.revision !== input.expected_revision) throw new ApiError(409, "conflict", "source changed");
        if (method === "DELETE") { this.knowledgeSources.set(bot, sources.filter((item) => item !== source)); return ok(undefined); }
        if (seg[5] === "reindex") return ok({ source, indexing: { processed: 1, unchanged: 1, removed: 0, next_cursor: null, failures: [] } });
        if (seg[5] === "embed") {
          if (settings.embedding?.provider_id === "local-onnx") return ok({ status: "failed", embedded_chunks: 0, cache_hits: 0, skipped_chunks: 0, remaining_chunks: 1, next_attempt_ms: null, failure_code: "local_embeddings_unavailable" });
          return ok({ status: source.remote_profile_fingerprint ? "complete" : "awaiting_consent", embedded_chunks: source.remote_profile_fingerprint ? 1 : 0, cache_hits: 0, skipped_chunks: 0, remaining_chunks: source.remote_profile_fingerprint ? 0 : 1, next_attempt_ms: null, failure_code: null });
        }
        if (input.action === "revoke_remote") source.remote_profile_fingerprint = null;
        if (input.action === "disable") { source.enabled = false; source.remote_profile_fingerprint = null; }
        if (input.action === "enable") { if (source.requires_path_selection) throw new ApiError(409, "conflict", "select a local path"); source.enabled = true; }
        if (input.action === "rebind" && input.path) { source.locator = input.path; source.requires_path_selection = false; source.enabled = true; source.remote_profile_fingerprint = null; source.documents = 0; source.chunks = 0; }
        if (input.action === "rename" && input.label) source.label = input.label;
        source.revision += 1;
        return ok(structuredClone(source));
      }
    }
    switch (`${method} /${seg[0] ?? ""}`) {
      case "GET /health":
        return ok({ status: "ok", uptime_secs: 3600, version: "0.1.0-mock" });
      case "GET /hosts":
        return ok({ local_version: "0.1.0-mock", local_api_version: "1", hosts: [] });
      case "GET /skills":
        return ok(structuredClone(MOCK_SKILLS_CATALOG));
      case "GET /bots":
        if (seg[1] && seg[2] === "skills") {
          return ok(this.getBotSkillsState(seg[1]));
        }
        return ok(structuredClone(this.bots));
      case "GET /chats":
        if (seg[2] === "messages") {
          if (this.messages.has(seg[1])) return ok(this.messages.get(seg[1]));
          if ((typeof window !== "undefined" && new URLSearchParams(window.location.search).get("screen") === "empty") || seg[1] !== CHAT_ID) return ok([]);
          const tr = typeof window !== "undefined" && new URLSearchParams(window.location.search).get("lang") !== "en";
          const content = tr ? [
            "Bu haftaki çalışma özetini hazırlar mısın? Tamamlanan işleri ve sonraki adımları görmek istiyorum.",
            "## Bu hafta güzel ilerledik.\n\nGitHub ve çalışma klasöründeki son değişiklikleri inceledim. İşte haftanın kısa özeti:\n\n| Çalışma | Tamamlanan | Durum |\n| --- | --- | --- |\n| Masaüstü deneyimi | Ayarlar, sohbet ve model seçimi | Hazır |\n| Entegrasyonlar | 5 uygulama bağlantısı | Hazır |\n| Kalite kontrolleri | Tip denetimi ve arayüz testleri | Tamamlandı |\n\n### Sıradaki adım\n\nAçık görevleri gözden geçirdikten sonra sürüm kontrollerini çalıştırabiliriz.\n\n```sh\npnpm typecheck\npnpm --dir apps/desktop/ui test\n```\n\nİstersen bu özeti ekibinle paylaşabileceğin bir rapora dönüştürebilirim.",
          ] : [
            "Can you prepare this week's work summary? I'd like to see what shipped and what's next.",
            "## A good week of progress.\n\nI reviewed the latest changes in GitHub and your workspace. Here is your weekly summary:\n\n| Workstream | Delivered | Status |\n| --- | --- | --- |\n| Desktop experience | Settings, chat and model selection | Ready |\n| Integrations | 5 connected apps | Ready |\n| Quality checks | Types and interface tests | Complete |\n\n### Next up\n\nAfter reviewing open tasks, we can run the release checks.\n\n```sh\npnpm typecheck\npnpm --dir apps/desktop/ui test\n```\n\nI can turn this into a report to share with your team.",
          ];
          const seeded = content.map((text, index): Message => ({ id: `mock-message-${index}`, chat_id: CHAT_ID, content: text, role: index % 2 === 0 ? "user" : "assistant", created_at_ms: NOW + index * 1000 }));
          this.messages.set(seg[1], seeded);
          return ok(seeded);
        }
        return ok(structuredClone(this.chats));
      case "POST /chats": {
        const input = opts.body as { bot_id?: string; title?: string; text?: string };
        if (seg[2] === "messages") {
          const previous = await this.request<Message[]>("GET", `/chats/${seg[1]}/messages`);
          this.messages.set(seg[1], [...previous, { id: crypto.randomUUID(), chat_id: seg[1], role: "user", content: input.text ?? "", created_at_ms: Date.now() } as Message]);
          return ok({ delivery: "started", run_id: crypto.randomUUID() });
        }
        const id = crypto.randomUUID();
        const created = { id, chat_id: id, bot_id: input.bot_id ?? ATLAS, title: input.title ?? "Yeni sohbet", created_at_ms: Date.now() };
        this.chats.push(created);
        this.messages.set(id, []);
        return ok(created);
      }
      case "GET /schedules":
        return ok(typeof window !== "undefined" && new URLSearchParams(window.location.search).get("screen") === "empty" ? [] : structuredClone(SCHEDULES));
      case "GET /approvals":
        return ok(typeof window !== "undefined" && new URLSearchParams(window.location.search).get("screen") === "approval"
          ? [{ call_id: "mock-rm", run_id: "mock-live", requested_at_ms: Date.now() - 3000, decision: null, tool_name: "shell.run", summary: "drafts/old klasörünü ve içindeki 14 dosyayı sil" }]
          : typeof window !== "undefined" && new URLSearchParams(window.location.search).get("screen")?.startsWith("approval-schedule")
          ? [{ call_id: "mock-sch", run_id: "mock-live", requested_at_ms: Date.now() - 3000, decision: null, tool_name: "schedule_create", summary: "Create a schedule" }]
          : []);
      case "GET /runs":
        if (seg[2] === "changes") {
          const runId = seg[1] ?? "";
          if (seg[4] === "preview") {
            return ok({
              run_id: runId,
              checkpoint_id: seg[3] ?? "all",
              entries: [],
              modified_since: [],
              covered: ["fs_write", "fs_edit", "fs_move", "fs_delete"],
              not_covered: ["shell", "web", "email", "desktop"]
            });
          }
          const withChanges =
            typeof window !== "undefined" &&
            new URLSearchParams(window.location.search).get("screen") === "changes";
          return ok({
            run_id: runId,
            changes: withChanges
              ? [{
                  accessible: true, after_size: 412, before_size: 380, checkpoint_id: "mock-cp-1",
                  current_matches_after: true, existed: true, is_dir: false, is_link: false,
                  kind: "modified", op: "fs_edit", path: "docs/haftalik-ozet.md", verification: "matched",
                  text: { added_lines: 2, removed_lines: 1, hunks: 1, unified: ["@@ -1,3 +1,4 @@", " # Haftalik ozet", "-Taslak", "+Hazir", "+Sonraki adim: surum kontrolleri", " Notlar", ""].join(String.fromCharCode(10)) }
                }]
              : [],
            next_offset: null,
            covered: ["fs_write", "fs_edit", "fs_move", "fs_delete"],
            not_covered: ["shell", "web", "email", "desktop"]
          });
        }
        {
          const live = mockLiveRunId();
          return ok(live ? [{
            id: live, bot_id: "11111111-1111-4111-8111-111111111111", chat_id: CHAT_ID,
            state: "running", created_at_ms: Date.now() - 6000, parent_run_id: null
          }] : []);
        }
      case "GET /usage":
        return ok(USAGE);
      case "GET /doctor": {
        const checks = structuredClone(this.doctorChecks);
        return ok({ ok: checks.every((c) => c.status !== "fail"), checks });
      }
      case "POST /doctor": {
        const id = seg[1] === "repair" ? (seg[2] ?? "") : "";
        if (!["create_data_dir", "reload_providers", "restart_browser"].includes(id)) {
          throw new ApiError(404, "not_found", `repair ${id} not found`);
        }
        this.repairs.push(id);
        for (const c of this.doctorChecks) {
          if (c.repair === id) {
            c.status = "ok";
            c.repair = null;
            c.hint = null;
            c.detail = "fixed";
          }
        }
        return ok({ repair: id, detail: "done" });
      }
      case "GET /providers":
        if (seg[1] === "cli") {
          return ok(this.detectedProvidersList.map((detected) => {
            const native = ["codex", "opencode", "gemini"].includes(detected.id);
            const entry = this.providersDto.configured.find((p) => p.id === `cli:${detected.id}`);
            const pkg = MOCK_NPM[detected.id];
            return {
              detected, executable_names: [detected.id], version_args: ["--version"], auth_args: [],
              path: detected.installed ? ["C:", "Users", "emre", "AppData", "Roaming", "npm", detected.id === "agy" ? "agy.exe" : `${detected.id}.cmd`].join(String.fromCharCode(92)) : null,
              minimum_version: null, outdated: false,
              login: detected.id === "claude" ? "claude auth login" : `${detected.id} login`,
              docs_url: "https://example.com/docs",
              install: pkg && !detected.installed ? { command: `npm install -g ${pkg}`, program: "npm", args: ["install", "-g", pkg], verified: true, executable: true } : null,
              native: native ? { support: detected.id === "gemini" ? "experimental" : "verified", levels: [] } : null,
              limitation: detected.runnable ? null : native ? "native_only" : "detection_only",
              enabled: entry?.enabled ?? false,
              user_disabled: entry ? !entry.enabled : false
            };
          }));
        }
        if (seg[1] === "detected") {
          const screen =
            typeof window !== "undefined"
              ? new URLSearchParams(window.location.search).get("screen")
              : null;
          if (screen === "detected-empty") {
            return ok(
              this.detectedProvidersList.map((row) => ({
                ...row,
                installed: false,
                version: null,
                auth: "unknown",
                account_label: null,
                runnable: false,
                detail: "not_on_path",
                message: null
              }))
            );
          }
          return ok(structuredClone(this.detectedProvidersList));
        }
        return ok(structuredClone(this.providersDto));
      case "POST /providers": {
        if (seg[1] === "cli" && seg[2] === "disable") {
          const id = ((opts.body ?? {}) as { id?: string }).id ?? "";
          const entry = this.providersDto.configured.find((p) => p.id === `cli:${id}`);
          if (entry) {
            entry.enabled = false;
            entry.active = false;
          }
          this.providersDto.active = this.providersDto.active.filter((a) => a !== `cli:${id}`);
          return ok(structuredClone(entry ?? { id: `cli:${id}`, enabled: false }));
        }
        if (seg[1] === "cli" && seg[2] === "enable") {
          const body = (opts.body ?? {}) as { id?: string };
          const id = body.id ?? "";
          if (!["claude", "codex", "opencode", "gemini"].includes(id)) {
            throw new ApiError(
              400,
              "bad_request",
              "This coding CLI is detected only and cannot be run in this version of Praxi."
            );
          }
          const providerId = `cli:${id}`;
          const existing = this.providersDto.configured.find(
            (p) => p.id === providerId
          );
          const entry: ConfiguredProviderDto = {
            id: providerId,
            enabled: true,
            is_default: existing?.is_default ?? false,
            default_model: existing?.default_model ?? null,
            has_key: false,
            custom: false,
            active: true,
            models: existing?.models ?? [],
            models_error: null
          };
          if (existing) {
            Object.assign(existing, entry);
          } else {
            this.providersDto.configured.push(entry);
          }
          if (!this.providersDto.active.includes(providerId)) {
            this.providersDto.active.push(providerId);
          }
          if (!this.providersDto.catalog.some((c) => c.id === providerId)) {
            this.providersDto.catalog.push({
              id: providerId,
              kind: "cli",
              base_url: "",
              auth: "none",
              key_env: null,
              tools: true,
              vision: false,
              configured: true
            });
          }
          return ok(structuredClone(entry));
        }
        return ok({
          default_provider: this.providersDto.default_provider,
          active: [...this.providersDto.active]
        });
      }
      case "GET /policy":
        return ok({
          preset: "approve-destructive",
          categories: {}
        } satisfies PolicyDto);
      case "POST /secrets":
        this.secrets.set(seg[1] ?? "", Date.now());
        return ok(undefined);
      case "POST /bots": {
        const input = opts.body as { name: string; config: unknown };
        if (this.bots.some((b) => b.name.toLowerCase() === input.name.toLowerCase())) {
          throw new ApiError(409, "conflict", `a bot named "${input.name}" already exists`);
        }
        const created = { id: crypto.randomUUID(), name: input.name, config: input.config, created_at_ms: Date.now(), version: 1 } as Bot;
        this.bots.push(created);
        return ok(created);
      }
      case "PUT /bots":
        if (seg[1] && seg[2] === "skills") {
          return ok(this.putBotSkillsState(seg[1], opts.body as PutBotSkillsInput));
        }
        { const index = this.bots.findIndex(b => b.id === seg[1]);
          if (index >= 0) this.bots[index] = { ...this.bots[index], ...(opts.body as Partial<Bot>), version: this.bots[index].version + 1 };
          return ok(this.bots[index] ?? this.bots[0]); }
      case "PATCH /bots": {
        const index = this.bots.findIndex((b) => b.id === seg[1]);
        const current = this.bots[index];
        if (!current) throw new ApiError(404, "not_found", "bot");
        const input = opts.body as { expected_version: number; name?: string; ops?: PatchOp[] };
        if (input.expected_version !== current.version) {
          throw new ApiError(409, "version_conflict", "the bot changed since `expected_version`", {
            error: { code: "version_conflict", message: "the bot changed since `expected_version`" },
            current: structuredClone(current)
          });
        }
        const next = {
          ...current,
          name: input.name ?? current.name,
          config: applyPatchOps(current.config, input.ops ?? []),
          version: current.version + 1
        } as Bot;
        this.bots[index] = next;
        return ok(structuredClone(next));
      }
      case "GET /email":
      case "POST /email":
      case "PUT /email":
      case "DELETE /email":
        return ok(this.email(method, seg, opts.body));
      case "GET /composio":
      case "POST /composio":
      case "PUT /composio":
      case "DELETE /composio":
        return ok(this.composio(method, seg, opts.body));
      case "GET /mcp":
      case "POST /mcp":
      case "PUT /mcp":
      case "DELETE /mcp":
        return ok(this.mcp(method, seg, opts.body));
      case "POST /runs":
        if (seg[1] === "stop") {
          // The mock has no live runs: every Stop targets nothing and is confirmed at once.
          const input = opts.body as { stop_id: string; run_ids?: string[] };
          // Review state `?screen=working|approval`: the live mock run winds down slowly, so the
          // "Stopping..." status stays on screen.
          const live = mockLiveRunId();
          const targets = live && (input.run_ids ?? [live]).includes(live) ? [live] : [];
          return ok({
            stop_id: input.stop_id, first_seen: true, audited: true,
            results: targets.map((run_id) => ({ run_id, outcome: "cancel_requested", state: "running" }))
          });
        }
        if (seg[2] === "changes" && seg[4] === "restore") {
          return ok({
            run_id: seg[1] ?? "",
            checkpoint_id: seg[3] ?? "all",
            partial: false,
            error: null,
            safety_checkpoint_ids: [],
            restored: [],
            warnings: [],
            covered: ["fs_write", "fs_edit", "fs_move", "fs_delete"],
            not_covered: ["shell", "web", "email", "desktop"]
          });
        }
        throw new ApiError(404, "not_found", `mock: ${method} ${path}`);
      case "GET /view":
      case "POST /view":
      case "PATCH /view":
      case "PUT /view":
      case "DELETE /view":
        return ok(await this.view(method, seg, opts.body));
      case "GET /control":
      case "PUT /control":
      case "POST /control":
        return ok(this.control(method, seg, opts.body));
      case "GET /web":
      case "PUT /web":
      case "POST /web":
        return ok(this.web(method, seg, opts.body));
      default:
        throw new ApiError(404, "not_found", `mock: ${method} ${path}`);
    }
  }

  private async view(
    method: string,
    seg: string[],
    body: unknown
  ): Promise<unknown> {
    const [, sub, id, action] = seg;
    if (sub === "targets" && method === "GET") {
      return this.liveView.listTargets();
    }
    if (sub === "privileges" && method === "PUT" && id) {
      return undefined;
    }
    if (sub === "sessions") {
      if (!id && method === "POST") {
        return this.liveView.createSession(body as CreateSessionInput);
      }
      if (id && !action) {
        if (method === "PATCH") {
          return this.liveView.updateSession(id, body as SessionUpdate);
        }
        if (method === "DELETE") {
          this.liveView.deleteSession(id);
          return undefined;
        }
      }
      if (id && action === "heartbeat" && method === "PUT") {
        this.liveView.heartbeatSession(id, body as ViewHeartbeat);
        return undefined;
      }
      if (id && action === "input" && method === "POST") {
        return await this.liveView.sendInput(id, body as InputBatch);
      }
    }
    throw new ApiError(404, "not_found", `mock view: ${method} /${seg.join("/")}`);
  }

  private control(method: string, seg: string[], body: unknown): unknown {
    const [, sub] = seg;
    if (sub === "state") {
      if (method === "GET") {
        return this.liveView.getControlState();
      }
      if (method === "PUT") {
        const input = (body ?? {}) as {
          mode: ControlModeName;
          release?: boolean;
        };
        return this.liveView.putControlState(input.mode, Boolean(input.release));
      }
    }
    if (sub === "takeover" && method === "POST") {
      return this.liveView.requestTakeover(body as TakeoverRequest);
    }
    if (sub === "release" && method === "POST") {
      return this.liveView.releaseControl(body as ReleaseRequest | undefined);
    }
    if (sub === "override" && method === "POST") {
      return this.liveView.overrideControl();
    }
    if (sub === "recover" && method === "POST") {
      return this.liveView.recoverControl();
    }
    if (sub && method === "POST") {
      const verdict = (body ?? {}) as { decision?: string };
      return { request_id: sub, decision: verdict.decision ?? "proceed" };
    }
    throw new ApiError(404, "not_found", `mock control: ${method} /${seg.join("/")}`);
  }

  private getBotSkillsState(botId: string): BotSkills {
    if (!this.bots.some((b) => b.id === botId)) {
      throw new ApiError(404, "not_found", `bot ${botId} not found`);
    }
    const stored = this.botSkills.get(botId);
    const mode: SkillAssignmentMode = stored?.mode ?? "all";
    const names = mode === "selected" ? stored?.names ?? [] : [];
    const availableByName = new Map(
      MOCK_SKILLS_CATALOG.skills.map((s) => [s.name, s])
    );
    const selectedSet = new Set(names);
    return {
      version: stored?.version ?? 0,
      mode,
      assigned: names.map((name) => ({
        name,
        state: availableByName.has(name) ? "ok" : "missing"
      })),
      available: MOCK_SKILLS_CATALOG.skills.map((s) => ({
        ...structuredClone(s),
        assigned: mode === "all" || selectedSet.has(s.name)
      })),
      own: structuredClone(MOCK_OWN_SKILLS[botId] ?? [])
    };
  }

  private putBotSkillsState(botId: string, body: PutBotSkillsInput): BotSkills {
    if (!this.bots.some((b) => b.id === botId)) {
      throw new ApiError(404, "not_found", `bot ${botId} not found`);
    }
    const current = this.botSkills.get(botId)?.version ?? 0;
    if (body.expected_version != null && body.expected_version !== current) {
      throw new ApiError(
        409,
        "conflict",
        `bot skill assignment changed (expected version ${body.expected_version}, current ${current})`
      );
    }
    if (body.mode === "all") {
      this.botSkills.delete(botId);
      return this.getBotSkillsState(botId);
    }
    const seen = new Set<string>();
    const normalized: string[] = [];
    for (const raw of body.names ?? []) {
      if (!SKILL_NAME_RE.test(raw)) {
        throw new ApiError(422, "unprocessable_entity", `invalid skill name: ${raw}`);
      }
      if (!seen.has(raw)) {
        seen.add(raw);
        normalized.push(raw);
      }
    }
    if (normalized.length > 200) {
      throw new ApiError(422, "unprocessable_entity", "too many assigned skills");
    }
    this.botSkillsVersion += 1;
    this.botSkills.set(botId, {
      mode: "selected",
      names: normalized,
      version: this.botSkillsVersion
    });
    return this.getBotSkillsState(botId);
  }

  private email(method: string, seg: string[], body: unknown): unknown {
    const [, sub, name, action] = seg;
    if (sub === "test") {
      const draft = (body ?? {}) as EmailAccountInput;
      return draft.provider === "agentmail"
        ? { imap: "ok", smtp: "ok", ok: true, api: "ok" }
        : { imap: "ok", smtp: "ok", ok: true };
    }
    if (sub !== "accounts") {
      throw new ApiError(404, "not_found", "mock email");
    }
    if (!name) {
      if (method === "GET") {
        return this.emailAccounts;
      }
      const input = body as EmailAccountInput;
      if (this.emailAccounts.some((a) => a.name === input.name)) {
        throw new ApiError(409, "conflict", `account ${input.name} exists`);
      }
      if (input.provider === "agentmail") {
        // The "provider" creates the inbox; the key is write-only.
        const local = (input.name_hint ?? input.name)
          .toLowerCase()
          .replace(/[^a-z0-9]+/g, "-")
          .replace(/^-+|-+$/g, "");
        const address = `${local || "bot"}@agentmail.to`;
        const made: EmailAccount = {
          name: input.name,
          provider: "agentmail",
          inbox_id: address,
          address,
          username: "",
          imap: { host: "", port: 993 },
          smtp: { host: "", port: 465, security: "implicit" },
          drafts_folder: "Drafts",
          bot: input.bot ?? null,
          allowed_senders: input.allowed_senders ?? [],
          password_set: Boolean(input.api_key),
          custom_ca: false
        };
        this.emailAccounts.push(made);
        return made;
      }
      const imap = input.imap ?? { host: "" };
      const smtp = input.smtp ?? { host: "" };
      const created: EmailAccount = {
        name: input.name,
        provider: "imap_smtp",
        inbox_id: null,
        address: input.address ?? "",
        username: input.username ?? "",
        imap: { host: imap.host, port: imap.port ?? 993 },
        smtp: {
          host: smtp.host,
          port: smtp.port ?? 465,
          security: smtp.security ?? "implicit"
        },
        drafts_folder: input.drafts_folder ?? "Drafts",
        bot: input.bot ?? null,
        allowed_senders: input.allowed_senders ?? [],
        // The password is write-only: only the fact that one is stored is kept.
        password_set: Boolean(input.password),
        custom_ca: false
      };
      this.emailAccounts.push(created);
      return created;
    }
    const idx = this.emailAccounts.findIndex((a) => a.name === name);
    if (idx < 0) {
      throw new ApiError(404, "not_found", `no account ${name}`);
    }
    if (action === "test") {
      return this.emailAccounts[idx].provider === "agentmail"
        ? { imap: "ok", smtp: "ok", ok: true, api: "ok" }
        : { imap: "ok", smtp: "ok", ok: true };
    }
    if (method === "DELETE") {
      this.emailAccounts.splice(idx, 1);
      return undefined;
    }
    // PUT is a patch: a field left out keeps its value, `bot: null` = every bot.
    const patch = (body ?? {}) as Record<string, unknown>;
    const { password, api_key: apiKey, ...rest } = patch;
    const merged = {
      ...this.emailAccounts[idx],
      ...rest,
      password_set:
        this.emailAccounts[idx].password_set || Boolean(password || apiKey)
    } as EmailAccount;
    this.emailAccounts[idx] = merged;
    return merged;
  }

  private web(method: string, seg: string[], body: unknown): unknown {
    const [, sub, action] = seg;
    if (sub !== "search") {
      throw new ApiError(404, "not_found", "mock web");
    }
    if (action === "test" && method === "POST") {
      const input = (body ?? {}) as WebSearchTestInput;
      const backend =
        input.backend ?? this.webSearchConfig.active_backends[0] ?? null;
      return {
        ok: backend !== null,
        query: input.query?.trim() || "rust programming",
        backend,
        result_count: backend ? 5 : 0,
        elapsed_ms: 48,
        error: backend ? null : "no search backend configured",
        fallback_errors: []
      };
    }
    if (!action && method === "GET") {
      return structuredClone(this.webSearchConfig);
    }
    if (!action && method === "PUT") {
      const input = (body ?? {}) as WebSearchConfigInput;
      const cur = this.webSearchConfig;
      const order = input.order ?? cur.order;
      const searxng_url =
        input.searxng_url !== undefined
          ? (input.searxng_url ?? "").trim() || null
          : cur.searxng_url;
      const brave_key_set = input.clear_brave_key
        ? false
        : cur.brave_key_set || Boolean(input.brave_key);
      const tavily_key_set = input.clear_tavily_key
        ? false
        : cur.tavily_key_set || Boolean(input.tavily_key);
      const duckduckgo = input.duckduckgo ?? cur.duckduckgo;
      const duckduckgo_lite = input.duckduckgo_lite ?? cur.duckduckgo_lite;
      const bing = input.bing ?? cur.bing;
      const active_backends = order.filter((name) => {
        if (name === "searxng") return searxng_url !== null;
        if (name === "brave") return brave_key_set;
        if (name === "tavily") return tavily_key_set;
        if (name === "duckduckgo") return duckduckgo;
        if (name === "duckduckgo_lite") return duckduckgo_lite;
        if (name === "bing") return bing;
        return false;
      });
      this.webSearchConfig = {
        order,
        active_backends,
        searxng_url,
        brave_key_set,
        tavily_key_set,
        duckduckgo,
        duckduckgo_lite,
        bing
      };
      return structuredClone(this.webSearchConfig);
    }
    throw new ApiError(404, "not_found", "mock web");
  }

  private mockComposioKey = false;
  private mockComposioApps: ComposioApp[] = [];

  /** Review-state stand-in for `/v1/composio/*` (no network, no key). */
  private composio(method: string, seg: string[], body: unknown): unknown {
    const [, sub, slug, action] = seg;
    const status = { configured: this.mockComposioKey, key_setup_url: "https://platform.composio.dev/" };
    const kit = (s: string, name: string, cats: string[]): ComposioToolkit => ({
      slug: s, name, description: `${name} actions for your bots`, categories: cats, tools_count: 24,
      native_first: false, state: "not_connected", enabled: false
    });
    if (!sub) {
      return status;
    }
    if (sub === "key") {
      this.mockComposioKey = method === "PUT";
      return { ...status, configured: this.mockComposioKey };
    }
    if (sub === "toolkits") {
      const items = [kit("instagram", "Instagram", ["social"]), kit("linkedin", "LinkedIn", ["social"]),
        kit("twitter", "X", ["social"]), kit("hubspot", "HubSpot", ["crm"]), kit("gmail", "Gmail", ["email"]),
        kit("github", "GitHub", ["developer"])];
      return { popular: items.slice(0, 3), items, next_cursor: null };
    }
    if (sub === "apps" && !slug) {
      return structuredClone(this.mockComposioApps);
    }
    if (sub === "apps" && slug) {
      const i = this.mockComposioApps.findIndex((a) => a.slug === slug);
      if (action === "connect") {
        const app: ComposioApp = { slug, name: slug, state: "active", enabled: true, native_first: false, bots: null, tools: 6 };
        if (i < 0) { this.mockComposioApps.push(app); }
        return { redirect_url: "https://connect.composio.dev/link/mock", expires_at: "" };
      }
      if (i < 0) {
        throw new ApiError(404, "not_found", `app ${slug} not found`);
      }
      if (method === "DELETE") {
        this.mockComposioApps.splice(i, 1);
        return { remote_removed: true };
      }
      if (action === "enabled") {
        this.mockComposioApps[i] = { ...this.mockComposioApps[i], enabled: (body as { enabled: boolean }).enabled };
      }
      return this.mockComposioApps[i];
    }
    throw new ApiError(404, "not_found", "mock composio");
  }

  private mcp(method: string, seg: string[], body: unknown): unknown {
    const [, sub, name, action] = seg;
    if (sub === "catalog") {
      return MOCK_MCP_CATALOG;
    }
    if (sub !== "servers") {
      throw new ApiError(404, "not_found", "mock mcp");
    }
    if (!name) {
      if (method === "GET") {
        return structuredClone(this.servers);
      }
      const cfg = body as McpServerConfig;
      if (this.servers.some((s) => s.name === cfg.name)) {
        throw new ApiError(409, "conflict", `server ${cfg.name} exists`);
      }
      const s = serverFromConfig(cfg);
      this.servers.push(s);
      return s;
    }
    const idx = this.servers.findIndex((s) => s.name === name);
    if (idx < 0) {
      throw new ApiError(404, "not_found", `no server ${name}`);
    }
    if (action === "tools") {
      return MCP_TOOLS;
    }
    if (action === "reconnect") {
      this.servers[idx] = { ...this.servers[idx], status: "connected", error: null };
      return this.servers[idx];
    }
    if (method === "DELETE") {
      this.servers.splice(idx, 1);
      return undefined;
    }
    // PUT is a patch: a field left out keeps its value, `bots: null` = all bots.
    const patch = (body ?? {}) as Record<string, unknown>;
    const prev = this.servers[idx];
    const merged: McpServer = {
      ...prev,
      ...(typeof patch.enabled === "boolean" ? { enabled: patch.enabled } : {}),
      ...(typeof patch.category === "string" ? { category: patch.category } : {}),
      ...(typeof patch.command === "string" ? { command: patch.command } : {}),
      ...(typeof patch.url === "string" ? { url: patch.url } : {}),
      ...("bots" in patch ? { bots: (patch.bots as string[] | null) ?? null } : {})
    };
    // Atomic id operations; an open server (null) stays open.
    if (merged.bots != null) {
      let list = [...merged.bots];
      for (const id of (patch.bots_add as string[] | undefined) ?? []) {
        if (!list.includes(id)) {
          list.push(id);
        }
      }
      const rm = (patch.bots_remove as string[] | undefined) ?? [];
      list = list.filter((id) => !rm.includes(id));
      merged.bots = list;
    }
    this.servers[idx] = merged;
    return merged;
  }
}

/** The id of the live mock run for `?screen=working|approval` (review states), else null. */
function mockLiveRunId(): string | null {
  if (typeof window === "undefined") return null;
  const screen = new URLSearchParams(window.location.search).get("screen");
  return screen === "working" || screen === "approval" || screen === "subagents" ? "mock-live" : null;
}

/** Review-state events for `?mock=1&screen=...` (approval, streaming, default run). */
export function mockReviewEvents(): { ts_ms: number; event: GlobalEnvelope["event"] }[] {
  const out: { ts_ms: number; event: GlobalEnvelope["event"] }[] = [];
  const screen = new URLSearchParams(window.location.search).get("screen");
  if (screen === "approval" || screen?.startsWith("approval-schedule") || screen === "streaming" || screen === "working" || screen === "subagents") {
    const bot_id = "11111111-1111-4111-8111-111111111111";
    const chat_id = "33333333-3333-4333-8333-333333333333";
    const events: GlobalEnvelope["event"][] = [
      { type: "run_started", run_id: "mock-live", bot_id, chat_id },
      { type: "message_delta", run_id: "mock-live", text: screen === "approval" ? "Raporu hazırladım. Paylaşmadan önce eski taslakları temizlemem gerekiyor." : "Taslağı oluşturuyorum, önce kaynakları karşılaştırıyorum…" },
      ...(screen === "working" ? [
        { type: "tool_call_requested", run_id: "mock-live", call_id: "mock-w1", tool_name: "web_search", input: { query: "cpu llm benchmark 2026" } },
        { type: "tool_call_completed", run_id: "mock-live", call_id: "mock-w1", error: null },
        { type: "message_delta", run_id: "mock-live", text: "Kaynakları okuyup ölçümleri karşılaştırıyorum." },
        { type: "tool_call_requested", run_id: "mock-live", call_id: "mock-w2", tool_name: "web_extract", input: { url: "https://huggingface.co/blog/cpu-inference" } },
        { type: "tool_call_completed", run_id: "mock-live", call_id: "mock-w2", error: null },
        { type: "tool_call_requested", run_id: "mock-live", call_id: "mock-w3", tool_name: "github.search_issues", input: { query: "gguf arm" } }
      ] as GlobalEnvelope["event"][] : []),
      ...(screen === "subagents" ? [
        { type: "subagent_started", run_id: "mock-sub-1", parent_run_id: "mock-live", task: "GitHub'da son haftanın birleşen PR'larını tara", call_id: "mock-s1" },
        { type: "subagent_started", run_id: "mock-sub-2", parent_run_id: "mock-live", task: "Özeti ekibe e-postayla gönder", call_id: "mock-s2" },
        { type: "subagent_started", run_id: "mock-sub-3", parent_run_id: "mock-live", task: "CI sonuçlarını kontrol et", call_id: "mock-s3" },
        { type: "tool_call_requested", run_id: "mock-sub-1", call_id: "mock-s1a", tool_name: "find_tool", input: { query: "github" } },
        { type: "tool_call_completed", run_id: "mock-sub-1", call_id: "mock-s1a", error: null },
        { type: "tool_call_requested", run_id: "mock-sub-1", call_id: "mock-s1b", tool_name: "github.search_issues", input: { query: "is:pr is:merged" } },
        { type: "tool_call_requested", run_id: "mock-sub-2", call_id: "mock-s2a", tool_name: "email_send", input: { to: "ekip@praxi.dev" } },
        { type: "approval_needed", run_id: "mock-sub-2", call_id: "mock-s2a", tool_name: "email_send", input: { to: "ekip@praxi.dev" }, summary: "Haftalık özet e-postasını gönder" },
        { type: "tool_call_requested", run_id: "mock-sub-3", call_id: "mock-s3a", tool_name: "web_extract", input: { url: "https://ci.praxi.dev/runs/main" } },
        { type: "tool_call_completed", run_id: "mock-sub-3", call_id: "mock-s3a", error: null },
        { type: "message_delta", run_id: "mock-sub-3", text: "Tüm kontroller yeşil." },
        { type: "subagent_finished", run_id: "mock-sub-3", parent_run_id: "mock-live", state: "succeeded", summary: "CI son 24 saatte 12 çalışmada tamamen yeşil." }
      ] as GlobalEnvelope["event"][] : []),
      ...(screen?.startsWith("approval-schedule") ? [
        { type: "tool_call_requested", run_id: "mock-live", call_id: "mock-sch", tool_name: "schedule_create", input: {} },
        { type: "approval_needed", run_id: "mock-live", call_id: "mock-sch", tool_name: "schedule_create", input: {}, summary: "Create a schedule", details: { kind: "schedule", action: "create", name: "Pazartesi özeti", prompt: screen === "approval-schedule-long" ? LONG_PROMPT : "Geçen haftanın GitHub ve takvim hareketlerinden kısa bir özet hazırla ve bana gönder.", prompt_chars: screen === "approval-schedule-long" ? LONG_PROMPT.length : 76, previous: null, schedule: "Her pazartesi 09:00", timezone: "Europe/Istanbul", next_runs: ["2026-11-16T09:00:00+03:00", "2026-11-23T09:00:00+03:00"], permission: { level: "mixed", allow: ["github.*"], ask: ["email_send"], deny: [] } } }
      ] as GlobalEnvelope["event"][] : []),
      ...(screen === "approval" ? [
        { type: "tool_call_requested", run_id: "mock-live", call_id: "mock-rm", tool_name: "shell.run", input: { command: "rm -rf ./drafts/old" } },
        { type: "approval_needed", run_id: "mock-live", call_id: "mock-rm", tool_name: "shell.run", input: { command: "rm -rf ./drafts/old" }, summary: "drafts/old klasörünü ve içindeki 14 dosyayı sil" }
      ] as GlobalEnvelope["event"][] : [])
    ];
    events.forEach((event, i) => out.push({ ts_ms: Date.now() - 4000 + i * 300, event }));
  } else if (screen !== "empty") {
    const runId = screen === "changes" ? "mock-changes-run" : "mock-review-run";
    const events: GlobalEnvelope["event"][] = [
      { type: "run_started", run_id: runId, bot_id: "11111111-1111-4111-8111-111111111111", chat_id: "33333333-3333-4333-8333-333333333333" },
      { type: "tool_call_requested", run_id: runId, call_id: "mock-tool", tool_name: screen === "changes" ? "fs_edit" : "github.search", input: screen === "changes" ? { path: "docs/haftalik-ozet.md" } : { query: "is:closed updated:this-week" } },
      { type: "tool_call_completed", run_id: runId, call_id: "mock-tool", error: null },
      { type: "run_finished", run_id: runId, error: null }
    ];
    events.forEach((event, i) => out.push({ ts_ms: 1750000000000 + (i === 3 ? 2000 : 200 + i * 100), event }));
  }
  return out;
}
