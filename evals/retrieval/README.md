# Hermetic retrieval fixtures

These are fictional engineering notes, not statements about deployed Praxi or
WPanel installations. The corpus deliberately mixes Turkish and English,
Markdown headings, fenced code, tables, overlapping vocabulary and irrelevant
documents. No private files, real mail, keys or network responses are used.

Run `cargo test -p memory-store --test retrieval_eval -- --nocapture`.

The test reports source-deduplicated recall@5 and reciprocal rank over positive
queries. A separate no-match query checks abstention. Expected source filenames
are in `queries.json`; citation ranges and catalog scope are checked separately
in `knowledge.rs`. PR2 runs the same corpus through its deterministic fake
embedder and hybrid retriever; that is wiring evidence, not a benchmark of Gemini
or another semantic model.

The fake embedder constructs a sorted vocabulary from corpus text and returns
binary term vectors. It does not read filenames, source IDs, expected answers or
query labels. An orthogonal unknown-token coordinate makes the no-match case
exercise actual zero-similarity abstention. Both lexical and hybrid paths use
the same unchanged query labels and quality gates. The hybrid test additionally
checks durable indexing reuse and the bounded query-vector cache. Its scores
verify vector indexing, cosine ranking, fusion and result budgets; they do not
establish paraphrase or multilingual quality for a production embedding model.

The token comparison counts **complete serialized search responses**, including
citations, provenance and escaping, with the embedded `cl100k_base` reference BPE.
It also reports the runtime `ceil(Unicode scalars / 4)` estimate and UTF-8 bytes.
The comparison baseline concatenates this whole corpus as hypothetical memory
context. Today's shipped default has memory disabled, so it costs zero memory
tokens. The benchmark must never be described as measured production savings
over that default, or as the billed token count of a Gemini model. Agent-core
also measures the full request/tool interaction overhead using this corpus.

The corpus and thresholds are versioned. Changing expected sources to make a
failed search pass is not an acceptable fix without reviewing the query's
meaning. Duplicate chunks from one document do not count as independent relevant
sources. No request usage value from a scripted provider is used as measurement.

## Example measured run

The cumulative PR3 Linux run on 2026-11-14 used the unchanged 12-source fixture
corpus, 54 chunks, 11 positive queries and one separate no-match query. Both
quality tests and the full-request comparison passed. Generated UUIDs affect BPE
counts, so these are observed samples, not exact golden token counts.

| Measurement | Lexical | Deterministic hybrid |
| --- | ---: | ---: |
| Source-deduplicated Recall@5 | 1.000 | 1.000 |
| MRR over positive queries | 1.000 | 1.000 |
| Mean complete-result `cl100k_base` tokens | 704 | 707 |
| Maximum complete-result `cl100k_base` tokens | 930 | 970 |
| Same whole-corpus stuffing baseline | 4,816 | 4,816 |

Hybrid indexing embedded 54 document inputs and 12 query inputs. Repeated indexing
and repeated searches reused the respective caches; the fake embedding dimension
was 1,288. This demonstrates indexing/fusion/cache behavior, not real-model
paraphrase or multilingual quality.

The agent test counts the entire serialized test `ChatRequest`, including the
knowledge tool schema, its call, result, citations and escaping. Its control
removes that tool schema and inserts the same complete corpus as hypothetical
memory context. It measured:

| Reference tokenizer | Bounded retrieval request | Whole-corpus request | Reduction |
| --- | ---: | ---: | ---: |
| `cl100k_base` | 1,105 | 5,190 | 78.7% |
| `o200k_base` | 1,119 | 4,862 | 77.0% |

Reproduce that comparison with
`cargo test -p agent-core --lib knowledge::tests::complete_rag_request_uses_fewer_reference_tokens_than_stuffing_same_corpus -- --exact --nocapture`.
Its gate requires more than 50% reduction under both tokenizers while retaining
the relevant source. These figures are not vendor billing, production-wide
turn averages, or savings over the shipped `memory: None` baseline. Available
tool definitions still add their ordinary schema overhead even when automatic
retrieval is disabled.
