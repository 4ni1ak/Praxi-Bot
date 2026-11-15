-- Optimistic concurrency for bot profile edits (PATCH /v1/bots/{id}).
-- Every write to name/config bumps it.
ALTER TABLE bots ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
