# Model picker metadata

The daemon annotates the model ids returned by each configured provider. The
curated table is presentation data: it does not limit custom ids, alter a bot's
saved model, change capability overrides, or change pricing.

## API

`GET /v1/providers` keeps `configured[].models` as a string array. Its ids are
now filtered to chat candidates and ordered by recommendation rank, then display
name. The additive `configured[].model_details` array uses the same order:

```json
{
  "id": "claude-sonnet-5-5",
  "display_name": "Claude Sonnet 5.5",
  "kind": "chat",
  "recommended": true,
  "recommended_rank": 1,
  "deprecated": false,
  "note": null
}
```

For a compact picker, show entries with `recommended: true` first and put the
remaining entries behind an **All models** expander. The backend does not cap the
rest of the list. The existing desktop picker consumes the display names and
rank ordering; layout and the expander belong to the separate UI redesign.

Use `GET /v1/providers?chat_only=false` to receive non-chat and deprecated models
as well. `chat_only=true` is the default. The endpoint and peer permissions are
unchanged. No new Tauri command is introduced.

`kind` is the primary use: `chat`, `reasoning`, `vision_capable`, `embedding`,
`image`, `audio`, `video`, or `other`. The first three are chat candidates. A
recommended model has kind `chat` even when it also accepts images or reasons;
execution capability flags remain their own source of truth. Unknown ids get
a humanized display name and never get a recommendation. Conservative inference
hides names indicating image/video/audio/music/embedding/realtime generation,
specialized deep-research, computer-use, moderation, and reranking endpoints.

The API lists only ids actually returned by the provider. It does not inject a
curated model absent from the live listing. Local models therefore need to be
installed and visible to the local server first. A failed listing returns no
model candidates and a redacted error. Malformed or secret-bearing ids are
omitted rather than changed into unusable ids. A bot's current or manually
entered custom model remains usable even when absent from this list.

## Curated data and maintenance

`crates/provider-catalog/src/model_metadata.json` contains the checked-in table,
verification date, and source URLs. Its validator requires unique provider ids
and aliases, unique model ids within a provider, unique ranks in 1–4, no more
than four recommendations per provider, and no deprecated or non-chat
recommendations. Changing this file does not change inference or execution.

Verified on 2026-11-15:

| Provider | Candidate recommendations | Source |
| --- | --- | --- |
| Gemini and Gemini OpenAI compatibility | Gemini 4 Carbon, Argon, Hydrogen; Gemini 3.5 Flash-Lite | [Google model list](https://ai.google.dev/gemini-api/docs/models) and [deprecations](https://ai.google.dev/gemini-api/docs/deprecations) |
| OpenAI | GPT-6.1 Sol, GPT-6 Luna, GPT-6 Astra, GPT-6.1 Terra | [OpenAI model list](https://developers.openai.com/api/docs/models) |
| Anthropic | Claude Sonnet 5.5, Opus 5.5, Haiku 5.5, Fable 5.2 | [Claude model overview](https://platform.claude.com/docs/en/models/overview) |
| OpenRouter | Claude Sonnet 5.5, GPT-6 Luna, Gemini 3.8 Flash | [Sonnet](https://openrouter.ai/anthropic/claude-sonnet-5.5), [Luna](https://openrouter.ai/openai/gpt-6-luna), [Flash](https://openrouter.ai/google/gemini-3.8-flash) |
| xAI | Grok 4.7 | [xAI model list](https://docs.x.ai/developers/models) |
| DeepSeek | DeepSeek V4.1 Flash (`deepseek-flash`), V4 Pro | [DeepSeek model list](https://api-docs.deepseek.com/quick_start/pricing/) |
| Ollama | Qwen 3 8B, 4B, 30B; Llama 3.3 70B | [Qwen 3](https://ollama.com/library/qwen3), [Llama 3.3](https://ollama.com/library/llama3.3) |
| LM Studio | No static recommendations: ids depend on installed models and aliases | [Server-visible model list](https://lmstudio.ai/docs/developer/openai-compat/models) |
| Claude Code | Metadata for `default`, `sonnet`, `opus`, `haiku` | [Claude Code model configuration](https://code.claude.com/docs/en/model-config) |
| Codex | Metadata for `default` | [Codex models](https://developers.openai.com/codex/models); existing Praxi CLI runtime |

OpenAI and xAI currently use custom OpenAI-compatible provider configuration on
main; this change supplies metadata, not new provider adapters. Only Claude and
Codex have runtime engines; Cursor/OpenCode/Gemini CLI are detection-only on
main, so no new executable provider or guessed model ids are introduced.

CLI engines have no live model-list API in Praxi. They retain the configured
model or the existing `default` option; other CLI aliases in the table are not
injected as claims of account entitlement. xAI and DeepSeek intentionally have
fewer than three recommendations because their verified current chat lineups
are shorter. OpenRouter's three candidates use its verified exact ids, not
first-party ids. No `:free` model is pinned as a recommendation or default. A
`free` note is added only when an OpenRouter `:free` id appears in the live list.

## Limits of a model listing

A live model list and curated recommendation cannot prove that an account has
remaining quota, sufficient credit, or permission to invoke every returned id.
No paid completion or real-account probe is made by this feature. Provider
errors and per-account quota remain runtime concerns. Unknown model-name
inference is conservative, not a replacement for future provider-supplied
modality/entitlement metadata.

Gemini 2.5 Flash-Lite is **not deprecated** according to Google's current list;
it carries `note: "existing users only"`. This distinction matters even though
new projects should select a current recommended model. Existing bots keep
their configured id.

## Owner verification

1. Open a configured provider in the app and confirm human names are shown;
   selecting a name still saves the original provider/model id.
2. Compare `GET /v1/providers` with `?chat_only=false`: the default omits media,
   embeddings, specialized endpoints, and known deprecated models.
3. Stop a local model server and refresh: the API reports its listing failure,
   and no uninstalled recommendations appear.
4. Keep a bot on a custom model absent from the curated table and run a normal
   turn; metadata does not change its executor.

No real provider completion, installed desktop launch, or account quota check
is part of the automated tests; all model and route fixtures are local fakes.
