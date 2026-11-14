# Bot knowledge and retrieval

Status: implementation in three dependent `rag/*` PRs, all targeting `main`.
Baseline inspected: `a5a4f51bff99a75f566651a64c1ad240b9a0158c`.
Nothing in this design authorizes a merge or changes the agent-router workstream.

## Existing behavior and the problem to solve

[The baseline investigation](../discovery-current-memory.md) records exact source
anchors and read/write timing. `memory-store` already has learned memories,
FTS5, sqlite-vec, RRF, trust labels, undo journaling, session search and an
optional ONNX embedder. It does not have a document source catalog or a shipped
knowledge surface. Its synchronous embedder is mandatory even when only lexical
retrieval is wanted; a failed query embedding aborts lexical retrieval too.

The normal CLI constructs `LoopConfig` with `memory: None`: today's default adds
**zero learned-memory tokens and zero memory-extraction calls**. When explicitly
configured, legacy recall prepends a block to the first **user** message, once
per root turn, with an approximate 800-token content budget. It is included again
in later model calls in that run. Wrappers/escaping are not in that old budget.
Background extraction adds one model call plus a consolidation call for each
candidate with similar memories; those calls have no fixed prompt or candidate
budget and are not included in normal usage accounting. This proposal does not
enable that background extractor.

Other existing building blocks:

| Area | Existing behavior | Knowledge integration |
| --- | --- | --- |
| `storage` | Owned bots/chats/messages/runs and durable tool steps in `bot.db` | Resolve the authenticated bot and owner; select authorized conversation/email snapshots |
| `doc-tools` | Pure-Rust PDF extraction and ZIP/XML DOCX parsing; callers authorize paths | Read once through an authorized directory handle, then parse bounded bytes |
| `web-tools` | Bounded HTTP fetch and HTML-to-Markdown; heading, code and table preservation; untrusted output | Index saved HTML/text; adding a saved page makes no new network request |
| `skills` | Layered human-readable `SKILL.md` instruction library and bot-scoped prompt index | Keep the brain outside skills; retrieved notes never become skill instructions |
| `state-bundle` | Validates/snapshots three top-level SQLite databases; nested bot files were ordinary blobs | Explicitly snapshot and validate each knowledge database, omit WAL/SHM, revoke imported grants |
| API/UI | `rememberRun` saves a reusable workflow; no semantic-memory CRUD or knowledge settings | Bot-scoped sources/settings/search/indexing UI and generated API client |

PDF extraction has no OCR dependency: scanned/image-only PDFs can produce no
text. Citations for transformed PDF/DOCX/HTML refer to the extracted document
and say so; they must not invent original PDF page numbers or Word line numbers.

## Boundaries and storage

Knowledge is a module of `memory-store`, reusing the SQLite and ranking stack,
but a separate model/database from learned conversational facts. Learned-memory
soft deletion and undo snapshots cannot satisfy source erasure. Knowledge uses
hard source/document/chunk deletion and cascade cleanup of derived vectors.

Each physical index is `data_dir/bots/<canonical bot UUID>/knowledge.db`.
Its metadata permanently binds both `UserId` and `BotId`. Opening it with another
scope fails. The model cannot choose a bot ID, database path, owner or arbitrary
SQL in `knowledge_search`; execution supplies the bot from `ToolContext`, and the
host resolves its owner from `Storage`. Missing/invalid context fails closed.
All API routes independently check `bot_of_user` and source ownership.

Core entities:

| Entity | Stored fields / invariant |
| --- | --- |
| Source | Generated UUID, immutable kind/locator/provenance, label, enabled state, monotonic revision, optional explicit embedding grant, creation/update times |
| Document | Generated UUID, source UUID, stable source-relative locator, title, decoded format, content hash, mtime, revision |
| Chunk | Generated UUID, source/document UUIDs, ordinal, section ancestry, extracted line range, text and content hash |
| FTS | FTS5 `unicode61 remove_diacritics 2`; title/section/body weighted BM25; synchronized in the same transaction |
| Vector/cache (PR2) | Bot-local chunk/content identity plus full immutable embedding-profile fingerprint, validated normalized vector |
| Quota ledger (PR2) | Host-wide `knowledge-runtime.db`, keyed by provider/account group; no text, API keys or model output |

Source kinds are selected files/folders, saved web pages, conversations, email
snapshots, bot notes, and the shared brain. A shared source is explicitly attached
to each bot that may search it. There is no implicit all-bots index or fallback.
Private API credentials remain `SecretRef`s resolved through `secret-store`.

Portable knowledge indexes must have exactly the schema produced by the current
migrations, including triggers, foreign keys, indexes and FTS shadow tables.
Validation rejects additional or altered schema objects before import resets
source authority; an imported trigger cannot restore consent or suppress deletion.
Data and SQLite page layout may differ without changing the supported schema.

## Source acquisition, incremental work and limits

The source catalog is a grant to a specific source, not a grant inferred from
document frontmatter. Files/folders are human-selected roots. Authorized handles
constrain reads below those roots; symlink/reparse-point escapes, device/network
paths and nonregular inputs are refused. Parsing receives the authorized bytes
instead of reopening an unchecked path. Saved web pages accept saved content and
a validated HTTP(S) locator; they do not crawl or refresh that URL. A conversation
snapshot requires owner **and** bot/chat equality. Email snapshots come from an
authorized read by this bot and are rechecked against the account's current
binding. Account rebind/delete invalidates related sources.

Markdown chunking preserves heading ancestry, paragraphs, fenced code and table
headers. Large blocks split on complete lines where possible; pathological long
lines still have a hard bound. Code is indexed as code, with line citations.
Each content hash includes normalized format/chunker version as well as text;
mtime is metadata, not proof that bytes are unchanged. A changed document replaces
its previous chunks atomically. An unchanged document preserves chunk identity.
Source and document mutations require the expected source revision. A stale job
cannot overwrite a concurrent source change or resurrect a deleted source.

Defaults are deliberately finite: 4 MiB decoded document input, 1,600 characters
per chunk, 4,096 chunks per document, 256 sources, 50,000 documents and 100,000
chunks per bot. Source adapters impose their own input/extraction/scan limits
before invoking parsers. Indexing publishes one document at a time, reports a
cursor for bounded folder pages, and can restart safely through hash comparison.
Removed files are pruned only after a complete successful scan; a partial scan
must not delete unseen documents. Files larger than the limit are reported as
skipped/errors instead of being silently truncated into a complete-looking index.

CPU-heavy parsing/indexing runs off async executor threads with a shared bounded
worker permit. The permit stays with blocking work if its awaiting request is
cancelled. This avoids releasing capacity while a parser is still running.
Lexical search admits two concurrent blocking operations; indexing admits one.
No GPU, remote vector server, daemon-managed model download or telemetry is
required. These bounds are design constraints, not a benchmark claim for the
owner's ARM host; real throughput remains to be measured there.

## Retrieval and the agent loop

`knowledge_search` takes a plain query, optional source IDs within the current
bot, top-k and output budgets. FTS syntax is never accepted directly: query words
are quoted/normalized and joined as bounded alternatives. Empty or unmatched
queries return an empty result, without selecting arbitrary memories.

Default search returns at most five chunks with 600 **estimated** tokens and an
independent 8 KiB byte cap. Hard ceilings are 20 hits, 2,000 estimated tokens,
32 KiB response bytes and 256 lexical candidates. Budgeting serializes the
**complete** JSON response, including citations, escaping, truncation notices,
trust marker and the estimate field. It may shorten an excerpt or omit a hit
whose citation overhead alone cannot fit. Citation fields are source ID/label,
document locator, section and extracted line range. Approximate tokens are
labelled as estimates; they are not provider billing data.

PR2 adds bounded embedded vector retrieval and RRF. Vectors are scoped before
ranking; the old memory-store global top-N/post-filter limitation is not copied.
Vector storage is plain SQLite BLOBs with a bounded streaming cosine scan and
bounded top-k heap. This avoids another native ANN dependency and is appropriate
for a deliberately capped first implementation. A scan ceiling is explicit and
reported rather than pretending a capped scan searched the entire corpus.
The lexical index is authoritative and remains usable with no configured
embedder, absent consent, an exhausted quota, a timeout, malformed vectors or a
provider failure. RRF combines rank, not incomparable raw BM25/cosine scores.
A bounded local reranker may reorder the existing candidate set; a remote
reranker requires a separate egress authorization and is not enabled implicitly.

Automatic retrieval is **off by default**. `KnowledgeDecider` is our synchronous,
bounded hook for a heuristic or an already-computed decision from a future router
adapter. No `agent-router` code is changed. The shipped heuristic reads
`bot.config.knowledge.auto_retrieve`, skips commands/greetings, and schedules one
normal `knowledge_search` tool step before the first model call for a relevant
root-turn question. It adds no decider model call. Subagents and later model
steps do not silently repeat it. The same tool visibility, permission, approval,
timeout, output and audit machinery applies to explicit and automatic searches.
History reconstructs a matching assistant tool call and Tool-role result, so the
retrieved data never becomes a system instruction or an unmatched tool message.

## Trust, poisoning and PR #46

All retrieved text is `content_trust: untrusted_data`, including user-authored
Markdown. `source_trust` records provenance only; it cannot elevate a chunk to
an instruction. Titles, headings, paths, saved emails, quotes and frontmatter are
also untrusted text. The brain is never scanned as a skills directory.

Trusted daemon tool code supplies `OutputProvenance` independently of result
bytes. After execution actually starts, the agent persists an execution fact
with `ran`, `ingests_untrusted`, `reads_private` and a fixed `knowledge` label.
A denied/unexecuted call creates no exposure fact. Result text resembling JSON,
an XML system tag or a fake `fact` field cannot override this sideband. It survives
process restart because it is stored in the durable tool step.

PR #46 is a separate, **unmerged** egress workstream at the inspected baseline.
Its current `ToolSpec.content`, `ToolOutput.exposure` and persisted `ExecFact`
wire shapes are supported by a narrow constructor bridge on the new knowledge
tools. Main ignores the extra spec/output fields; #46 consumes them. A backend
that may send query embeddings declares egress before execution. The bridge
avoids changing unrelated tools or the permission engine. Main's provenance
tests establish metadata integrity, **not** an egress gate that does not yet
exist on main. End-to-end poisoning/egress enforcement requires #46 integration
and its gate tests on the combined code; merge review must preserve that boundary.
No prompt label alone is claimed to make an LLM immune to prompt injection.

Component integration was also run against exact #46 commit
`5d5773281bc38b1c08882bdbf3f8b6bb841efa87`: the identical knowledge tool/storage
and provenance code were registered on that branch with a test-only module.
Both direct search and discovery delegation persisted private/untrusted facts
despite a document forging `fact=false`; native egress approval then stopped a
canary-bearing `web_search` before invocation under `Policy::full`. Reopening
storage reconstructed the same taint and approval requirement. The native gate,
approval/execution modules and router remained byte-identical to #46. The
[reproducible harness](../../evals/retrieval/compatibility/README.md) records the
two passing tests. This checks the boundary contract, not a full branch merge or
the automatic-retrieval hook on a combined branch; main still needs #46.

At the inspected #46 head, egress classification is static per registered tool.
A mutable hybrid backend must conservatively declare egress even when a specific
call falls back to local search, so #46 will ask on tainted turns in that case.
The PR1 lexical backend declares no egress. Selective approval for a future hybrid
call needs a trusted invocation-specific classification seam; changing a prompt
label or allowing the backend to send after a non-egress classification is unsafe.

## Embeddings, caching and privacy

The only `provider-catalog` expansion is an embedding capability and factory,
using the existing Gemini/OpenAI-compatible providers, secret resolver and
sanitized HTTP error handling. Chat capabilities/model selection are unchanged.
Each embedding request has an explicit document/query role, model, task wire
format, dimensions and bounded input batch. Responses must match batch count,
indices, dimensions and finite nonzero values; invalid responses do not enter
the vector store. Embedding requests use one HTTP attempt and bounded response
bytes, refuse redirects, and never write failed document bodies to debug logs.

An embedding profile fingerprint binds provider, normalized destination,
model/revision, dimensions, task formatting and normalization version. Cache
identity also includes the exact formatted input/content hash and role. Models
with equal dimensions are not assumed compatible. Changing the profile requires
new consent and matching vectors; old vectors cannot silently answer queries.

Remote document indexing requires an explicit **per-source** grant for that
profile fingerprint. The model cannot create it with a tool argument or a
document annotation. Query embedding has a separate bot setting/grant: source
consent does not authorize sending arbitrary chat text. Reranking has no implied
remote grant. A missing/revoked grant leaves lexical retrieval available.
Indexing checks grant/revision before dispatch and again before publishing an
asynchronous result. Deletion/revocation prevents future publication; already
dispatched HTTP bytes cannot be recalled. No raw document cache is shared across
bots, and source deletion removes its chunks/vector associations and unused
derived cache records.

The daemon installs this runtime behind the same `knowledge_search` tool. It
reads the bot's current owner settings for every invocation and caches at most
16 bot/profile services. Each bot keeps only its current configuration snapshot;
changing or disabling settings releases the old service. Query-vector caches
hold hashes and at most 64 vectors for five minutes, never raw query text.
Before each remote attempt the host re-resolves the existing provider's current
`SecretRef` and destination. A cached service cannot send to a newly configured
endpoint under an old grant, and key rotation does not retain an old key.
The live secret redactor refreshes once before each outbound batch.

Source/settings mutations and bounded dispatch share an authority lock. A
management request reports busy while a previous dispatch is in flight; after
revocation succeeds, later dispatch cannot use its old settings snapshot.
Automatic continuation preserves durable retry limits. Only an explicit owner
retry resets a source's failed-job state; it never refunds account reservations
or clears provider cooldowns.

Gemini 001 uses `RETRIEVAL_QUERY`/`RETRIEVAL_DOCUMENT`. Gemini Embedding 2 instead
uses documented task prefixes and document title/text formatting; its multiple
content parts form an aggregate embedding, not independent vectors. A batch must
therefore preserve one independent request per input. The two model families use
different embedding spaces. These choices come from the [Gemini embedding
documentation](https://ai.google.dev/gemini-api/docs/embeddings), not model-name
guessing. [OpenAI-compatible embedding requests](https://platform.openai.com/docs/api-reference/embeddings/create)
use arrays of inputs and validate returned indices before reordering vectors.

The owner's approximate 100 RPM / 1,000 RPD are configurable account budgets,
not advertised universal quotas. A host-wide durable ledger reserves each
attempt, including retries, across bots sharing an account group. Batching,
cache hits, persistent backoff and `Retry-After` avoid retry storms. Conservative
rolling limits avoid assuming a provider's midnight boundary. The actual
[Gemini limits](https://ai.google.dev/gemini-api/docs/rate-limits) are project-scoped
and vary by account/model. A local ledger cannot coordinate unrelated machines
using the same external account; provider 429s remain authoritative.

The optional local path uses explicitly supplied model/tokenizer artifacts with
the existing permissively licensed fastembed/ONNX stack, where supported. It
must not infer permission to download missing artifacts. Multilingual E5 uses
different `query:`/`passage:` prefixes. Native artifact availability, model
license/checksums and Windows/ARM performance require separate validation; the
default build and CI retrieval eval never need a real model or a GPU.
Build this adapter with `cargo build -p cli --features local-embeddings` and
select six absolute, SHA-256-pinned files: runtime, ONNX model, tokenizer,
model config, special-token map and tokenizer config. The local recipe uses
up to eight tokenizer-checked windows per chunk, covers the complete input,
and pools normalized vectors with token weights. Oversized input is rejected
instead of silently truncating it. Inference uses one CPU thread and one
global inference slot with batches of two windows. Windows/Linux CI compiles
and tests the optional feature using hermetic artifact/tokenizer fixtures;
it does not download or run actual model weights.

## Shared Markdown brain and state transfer

The human-readable brain lives under `data_dir/brain`, outside `skills` and the
per-bot index. The first write operation creates a new Markdown note rather than
overwriting a human edit. `knowledge_brain_write` always asks for approval,
including under otherwise permissive policies. Before the card appears, it
freezes the exact rendered bytes, target relative path, source labels/revisions,
bot/run identity and hash in a bounded, expiring, one-use approval record. The
card shows the proposed content, not a vague summary. Changed arguments,
expired/reused approvals or changed source revisions fail. Capability-scoped
no-replace publication prevents traversal and accidental overwrite. Imported
frontmatter does not grant trust or write authority.

State bundles explicitly snapshot nested knowledge databases using SQLite's
backup API and validate scope/schema/foreign keys/index consistency before
installation. WAL and SHM files are not ordinary bundle blobs. Import disables
external source bindings, clears remote document/query consent and automatic
retrieval, and requires reauthorization on the destination. Pending approvals
and in-flight jobs are not transferable authority. The host quota ledger is
runtime state: it is excluded and an existing destination ledger must not be
rolled back by import. Shared Markdown files transfer as data and are only
indexed by bots that explicitly attach the brain.

## Evaluation, token accounting and delivery

The fixture corpus, queries and expected source IDs live under `evals/retrieval`.
Normal Rust tests run lexical recall@k/MRR and a deterministic fake-embedder
hybrid eval without network. Fake vectors test fusion, cache and isolation; they
are not evidence of Gemini/OpenAI semantic quality. Provider wire tests use local
mock HTTP responses. Real paid provider calls and model downloads are unnecessary.

Token tests report both the lightweight output estimate and real reference BPE
counts from embedded tokenizer assets. They compare the complete bounded result
and agent-request overhead against stuffing the **same complete fixture corpus**
into a prompt. This is an explicit counterfactual benchmark, because production
currently injects no memory by default. They must not claim that adding retrieval
reduces a zero-token baseline. Actual Gemini/other-provider billing and the
effect of prompt caching need live measurements outside these hermetic tests.

The delivery sequence is: (1) this design, scoped storage/chunking/lexical search,
tool/runtime hook and eval; (2) provider capability, consent-bound embeddings,
cache/quotas/vector/RRF/local option; (3) source management/OpenAPI/generated
client/UI, approved shared-brain writes and state-bundle transfer. Dependent PRs
remain reviewable as separate commits while all target the requested `main`.
No PR is merged by this task. Linux and Windows Rust jobs, full workspace
clippy/fmt/tests, cargo-deny and UI typecheck/tests/i18n/themes/lint are the gates.
CI results apply only to the exact PR heads tested. Native desktop interaction,
real embedding quality and the owner's ARM throughput are reported separately
from those gates.
