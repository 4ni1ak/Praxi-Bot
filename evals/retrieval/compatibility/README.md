# Knowledge / PR46 component compatibility

This is a reproducible component integration check, not a merge of the complete
RAG and egress branches. It runs the real knowledge tool against the existing
PR46 executor and permission gate. It does not add an egress gate to main.

## Pinned evidence

Observed on 2026-11-14:

| Input | Exact revision |
| --- | --- |
| PR46 native runtime / gate | `5d5773281bc38b1c08882bdbf3f8b6bb841efa87` |
| PR74 lexical knowledge components | `4542f8266c5f232e4dd38fa2e7d8bdf489af5a01` |
| PR74 complete source tree | `4b9d5d4632db44fc875d79755d8d9e2176c1d445` |

The observed local knowledge revision was `77d2f1a13b33387746ab4e4f93bc24019d3c3107`,
whose tree is identical to PR74's published tree above. The knowledge component
files were compared byte for byte before testing.

Both tests passed: **2 passed, 0 failed, 231 filtered out** (0.07 seconds after
compilation). The complete reproduction script was then run in another fresh
detached worktree and passed the same two tests (0.04 seconds), including the
unchanged-native-gate checks before and after execution. The two cases are:

- `direct_poisoned_knowledge_enters_real_egress_gate`.
- `discovery_poisoned_knowledge_enters_real_egress_gate`.

Each case indexes a fictional poisoned note with `PRIVATE-CANARY`, a fake system
tag and a forged `fact` declaring false exposure. A scripted model reads it
through the actual `knowledge_search` implementation and then tries `web_search`.
The second case uses `use_tool` discovery for both calls. With `Policy::full`,
PR46 still emits a real `ApprovalNeeded` card of kind `egress` with the complete
outgoing query, while the outgoing tool's execution count stays zero.

The test reopens `Storage` and checks the actual persisted execution fact:
`ran`, `reads_private` and `ingests_untrusted` are true, and the label is
`knowledge`. That native PR46 fact equals PR1's real
`OutputProvenance::executed()` value. Replaying the saved steps through PR46's
actual `observe_history_step` reconstructs exposure that makes the actual
`egress_must_ask` function return true. The document's forged JSON cannot replace
the trusted execution metadata.

## Reproduce

From a repository checkout with Rust and its normal native build dependencies:

```sh
git fetch origin 5d5773281bc38b1c08882bdbf3f8b6bb841efa87
git fetch origin 4542f8266c5f232e4dd38fa2e7d8bdf489af5a01
python evals/retrieval/compatibility/run_pr46_component.py \
  --repository "$PWD" \
  --worktree /tmp/praxi-rag46-component-check
```

Use a new worktree path on each run, or remove the previous detached worktree
yourself after reviewing it. The script refuses to reset an existing directory
and never writes a remote ref. `--target-dir` can select a dedicated Cargo output
directory. `PKG_CONFIG_PATH` and other native dependency settings are inherited.
The test fixtures make no network or paid-provider calls; an uncached build may
need ordinary Cargo dependency downloads.

The script copies the six `memory-store/src/knowledge` modules, the new knowledge
tool and its tests, and `tool-registry/src/provenance.rs` from the pinned PR74
commit. The only remaining overlay is module registration, the existing RAG
`sha2` / `memory-store` dependencies, the optional tool provenance trait hook,
and this test module. Cargo may update the temporary worktree's lockfile for
those dependencies. It then runs:

```sh
cargo test -p agent-core --lib rag_46_compat -- --nocapture
```

Before and after the run, it verifies that these native PR46 paths have no diff:

```sh
git diff --exit-code 5d5773281bc38b1c08882bdbf3f8b6bb841efa87 -- \
  crates/permission-engine \
  crates/agent-core/src/taint.rs \
  crates/agent-core/src/approval.rs \
  crates/agent-core/src/exec.rs \
  crates/agent-router
```

## What this does not establish

Main still lacks PR46's active gate. A complete future branch integration must
preserve that executor and gate behavior and rerun the combined runtime suite.
This component overlay does not combine the new automatic pre-retrieval hook,
post-redaction request budgeting, hybrid provider runtime, or source-management
API with PR46. Their separate PR tests remain necessary.

The scripted model deliberately attempts egress; this checks an enforced code
path, not a claim about a real model's resistance to poisoned instructions.
It does not measure embedding quality, ARM throughput, paid-provider billing,
or native Windows behavior.
