# MySQL transaction notes

## Deadlock retry

A deadlock means the whole transaction must be retried from its beginning.
Retry only the operation with an idempotency key, not an isolated final UPDATE
whose earlier reads are now stale. The transaction runner uses a bounded retry
count and randomized delay. Never hold a database transaction open while waiting
for a remote model, approval card, email server or external command.

The idempotency record and business update commit in one transaction. A repeated
request with the same key returns the previous recorded result. A conflicting
payload using that key is rejected; the runner must not silently substitute a
new amount or recipient. Logging the attempt number helps distinguish a transient
contention problem from a permanently invalid statement.

## Lock ordering

All paths acquire account rows in ascending identifier order. Read a row's
current version before computing a change, then include that version in the
conditional update. A zero-row update means a concurrent actor changed the
record and the caller needs a fresh read. Broad table locks are not a substitute
for making the ownership and version predicates explicit.

```sql
UPDATE project_settings
SET value_json = :value, version = version + 1
WHERE project_id = :project AND version = :observed_version;
```

## Migration review

Schema changes include an estimate of table size, lock duration and compatibility
with the previous application release. Add nullable fields before making the
application depend on them. A backfill runs in bounded primary-key ranges and
saves its cursor after a committed batch. The cursor names completed work, not a
promise that uncommitted rows have already been changed. Migration recovery must
be tested against interruption in the middle of a batch.
