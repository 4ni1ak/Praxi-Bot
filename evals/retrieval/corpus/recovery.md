# Interrupted workflow recovery

## Checkpoints

A workflow checkpoint records a completed, durable step and its result. Recovery
replays known read-only steps when safe, but never guesses that an interrupted
external write had no effect. A failed process can exit after the external system
accepted a change and before the local result was saved. Such a step needs an
idempotency key or an explicit observation before retry.

The recovery cursor advances only after the checkpoint is committed. A partially
prepared batch remains pending. Indexing one bounded document in a transaction
means a restart sees either its old complete set of chunks or its new complete
set, not a mixture of half-written revisions. Unchanged content hashes let the
next scan skip documents that were already committed.

## Cancellation

Cancellation is a state transition with an observable result. Long-running work
checks cancellation between bounded batches and drops unpublished intermediate
data. A later completion callback must not resurrect a cancelled job or restore
an old permission grant. The caller displays whether a step completed, was
cancelled before execution, or has an unknown external outcome.

## Operator review

The review screen identifies the last confirmed step, the outstanding action and
the evidence available for recovery. It avoids claiming success from the presence
of an intent record alone. Logs retain identifiers and status, while private
document bodies stay out of diagnostic messages. If the same failure repeats,
stop the automatic retry loop and preserve enough metadata for a manual repair.

## Durable settings

Changes to a source, profile or consent carry a monotonically increasing revision.
Background work captures that revision before starting and checks it before
publishing. The comparison and write share one transaction, closing the window
where a concurrent user action could invalidate an earlier check.
