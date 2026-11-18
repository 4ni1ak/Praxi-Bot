# Embedding work scheduler

## Persistent quota

Embedding quota is charged to the provider account, not independently to each
bot. The fixture profile sets a request-per-minute and daily request allowance.
Reservations are durable so restarting the process does not reset the daily
counter. A denied reservation leaves lexical search available and reports that
semantic indexing is paused. The scheduler does not repeatedly retry a daily
limit response in a tight loop.

## Batching and cache

Batch only texts for the same immutable embedding profile and an explicitly
authorized source. A profile fingerprint includes provider identity, canonical
endpoint, model revision, dimensions, document/query formatting and
normalization. The cache key includes that fingerprint and the content hash.
Changing a dimension or model requires new vectors; vectors with the same length
but different model meaning must never be mixed.

The scheduler checks source revision and consent before sending a batch and
again before committing results. If a source is deleted or its consent revoked
while the request is in flight, its returned vectors are discarded. A successful
remote response is not permission to recreate a deleted source.

## Query privacy

A source's document-embedding grant does not automatically grant permission to
send every conversational query to that provider. Query egress is configured
separately by the host. In the absence of a query grant, use lexical retrieval or
an explicitly local embedder. A local model may require a model-file download,
which is distinct from uploading private document contents for embedding.

## Small host limits

Use one indexing worker and small bounded batches. Exact vector scan reads
bounded rows and retains only its top candidates. If a configured scan limit is
exceeded, report lexical degradation instead of describing a partial scan as
complete global semantic search. No external vector server is required for this
fixture's small collection of notes.
