-- Per-bot persistent workspace mapping & optional quota, plus host disk warning settings.

CREATE TABLE bot_workspaces (
    -- Keep folder reservations after bot deletion, so a replacement bot cannot
    -- inherit another bot's persistent files by taking its display name.
    bot_id            TEXT PRIMARY KEY,
    user_id           TEXT NOT NULL REFERENCES users(id),
    folder_name       TEXT NOT NULL,
    soft_limit_bytes  INTEGER,
    hard_limit_bytes  INTEGER,
    created_at_ms     INTEGER NOT NULL,
    updated_at_ms     INTEGER NOT NULL
);

CREATE UNIQUE INDEX bot_workspaces_user_folder ON bot_workspaces(user_id, folder_name);

CREATE TABLE workspace_settings (
    user_id                TEXT PRIMARY KEY REFERENCES users(id),
    warn_percent_1         INTEGER NOT NULL DEFAULT 80,
    warn_percent_2         INTEGER NOT NULL DEFAULT 90,
    warn_percent_3         INTEGER NOT NULL DEFAULT 95,
    min_free_bytes         INTEGER NOT NULL DEFAULT 5368709120,
    downloads_max_age_days INTEGER DEFAULT 30,
    logs_max_age_days      INTEGER DEFAULT 30,
    updated_at_ms          INTEGER NOT NULL
);
