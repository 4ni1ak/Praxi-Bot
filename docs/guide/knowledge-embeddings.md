# Knowledge embedding setup

Knowledge search works locally without an embedding provider. Remote embeddings
and local native inference are optional host capabilities. Source management and
owner approval UI arrive in the sources layer of the RAG stack; the embedding
layer exposes the same host services for those routes.

## Remote providers and owner approval

Configure the provider and its SecretRef through the normal provider settings.
Choose its model, endpoint identity and dimensions in the bot's embedding
settings. Selecting a provider does not authorize document or query text to leave
the host. The daemon ignores a portable `account_group` and derives the shared
quota key from the provider's credential reference (or an unauthenticated
endpoint). Bots using the same credential reference share the ledger.

The owner confirms document egress separately for **each source**. Query egress
has a separate bot-wide confirmation because a query can contain private chat
text unrelated to a source. The approval card identifies the provider, model,
destination and source. Challenges expire after five minutes, accept one
confirmation, and reject changed configuration or source revision. General tool
grants and JSON consent fields cannot supply this authority.

Embedding uses the permission engine's existing `Remote` category. An effective
`Remote: deny` under its global/bot override rules, a policy lock, or an unsupported limit blocks the
confirmation and dispatch. That category also governs remote computer access;
changing it therefore affects both features. Query dispatch uses the actual
run's Human, Schedule or Channel initiator and inherited subagent policy. A Human
setup confirmation does not override a scheduled/channel Deny. The host keeps
the approved grant outside bot configuration in `knowledge-authority.db`.

Changing a model requires new confirmation. Old paid vectors remain available
as an inactive generation until a successful replacement completes; they cannot
silently answer queries for a different profile. Explicit source revocation or
disable removes its remote vectors. Import suspends sources, drops all imported
vector/cache state, and rotates the index identity so old host grants cannot be
replayed even with the same bot/source UUIDs.

Provider 429 responses defer work without consuming terminal retry attempts.
Every actual HTTP attempt still consumes the local quota. If the provider sends
an invalid or excessive retry delay, the persisted cooldown is bounded to 48
hours. The owner cooldown-reset operation clears provider waiting deadlines
across bots using that credential identity; it preserves the quota ledger.
Input rejection isolates and records skipped chunks so healthy chunks can
continue. Explicit retry clears the source's skipped-chunk and failure state.

## Local native profile registry

Build the daemon with the `local-embeddings` feature. Install the multilingual E5
small model, tokenizer JSON/config assets and a compatible ONNX Runtime library
from sources whose integrity and license you have verified. The daemon does not
download missing artifacts. Linux and Windows have supported immutable runtime
snapshots; local native loading fails closed on other operating systems.

Create `knowledge-host.json` directly in the daemon data directory. This is an
operator file, not a bot profile or an HTTP-editable setting. On Unix it must be
owned by the daemon user and not writable by group or others (mode `0600` is
appropriate). Paths below illustrate the shape; replace each path and every
`REPLACE_WITH_64_HEX_SHA256` string with the actual absolute path and SHA-256.
On Windows use absolute paths such as `C:\\PraxiModels\\onnxruntime.dll`.

```json
{
  "profiles": {
    "e5-small": {
      "label": "Multilingual E5 small on this host",
      "artifacts": {
        "runtime": {
          "path": "/opt/praxi-models/onnxruntime/libonnxruntime.so",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "model": {
          "path": "/opt/praxi-models/e5-small/model.onnx",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "tokenizer": {
          "path": "/opt/praxi-models/e5-small/tokenizer.json",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "config": {
          "path": "/opt/praxi-models/e5-small/config.json",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "special_tokens_map": {
          "path": "/opt/praxi-models/e5-small/special_tokens_map.json",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "tokenizer_config": {
          "path": "/opt/praxi-models/e5-small/tokenizer_config.json",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        }
      }
    }
  }
}
```

The registry accepts up to 16 profile IDs, each 1–128 ASCII letters, digits,
underscores or hyphens. The UI receives only ID, label and derived model profile;
the bot selects `local_profile_id: "e5-small"`. Bot JSON cannot override any
artifact path or hash. A final symlink/reparse point is rejected when opening the
registry, and checks apply to the handle whose bytes are read.

Model/tokenizer files are read into verified owned buffers. The native runtime
is copied into a sealed memfd on Linux or a private file held against
write/delete sharing on Windows. Native loading uses that snapshot, never the
original pathname. The process retains the verified runtime handle for its
lifetime; replacing the original installed files cannot change already admitted
bytes. Restart after changing the native runtime generation. Inference uses one
CPU thread and a single shared inference slot, with two windows per batch and at
most eight 512-token windows per input. Oversized inputs are isolated as skipped
chunks rather than silently truncated.

## Corpus limits and validation

The exact scanner admits a query only after checking vector count, dimensions
and estimated bytes. With the default 500 ms budget its byte limit is 16,384,000:
10,666 vectors at 384 dimensions, 2,666 at 1,536, or 1,333 at 3,072, additionally
bounded by the configured row limit. Larger eligible sets fall back to lexical
retrieval without spending a remote query request. These ceilings bound work;
they do not promise that every machine finishes within 500 ms.

CI verifies the resolved local-feature graph excludes model-hub and ONNX binary
download features. It fetches Cargo dependencies separately, then runs the
hermetic tests with Cargo offline mode and supported ONNX offline/skip-download
flags. This does not provide an operating-system network sandbox. CI fixtures
verify feature wiring, byte pinning, tokenizer/window behavior and cancellation;
real model quality and machine-specific inference speed require measurements
with the operator's installed artifacts.
