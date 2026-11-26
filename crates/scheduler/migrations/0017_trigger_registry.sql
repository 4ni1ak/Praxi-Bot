-- The registry extends schedules.db (independent of bot.db migrations).
CREATE TABLE trigger_registry (
    schedule_id TEXT PRIMARY KEY REFERENCES schedules(id) ON DELETE CASCADE,
    config_json TEXT NOT NULL,
    webhook_id TEXT UNIQUE,
    state_json TEXT NOT NULL DEFAULT '{"pending":[],"fired":[],"last_error":null}'
);
CREATE TABLE trigger_bot_rate (
    bot_id TEXT PRIMARY KEY,
    fired_json TEXT NOT NULL
);
