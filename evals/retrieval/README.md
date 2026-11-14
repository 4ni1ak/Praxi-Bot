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
