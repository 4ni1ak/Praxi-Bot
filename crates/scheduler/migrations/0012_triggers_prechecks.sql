-- Fifth scheduler migration; filename 0012 is reserved across concurrent feature PRs.
ALTER TABLE schedules ADD COLUMN precheck_json TEXT;
CREATE TABLE schedule_checks (
    schedule_id TEXT PRIMARY KEY REFERENCES schedules(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    last_hash TEXT,
    skipped_by_precheck INTEGER NOT NULL DEFAULT 0,
    precheck_errors INTEGER NOT NULL DEFAULT 0,
    trigger_errors INTEGER NOT NULL DEFAULT 0,
    watch_json TEXT
);
