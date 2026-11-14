CREATE TABLE bot_skill_assignment (
    bot_id        TEXT PRIMARY KEY REFERENCES bots(id),
    mode          TEXT NOT NULL CHECK (mode IN ('all', 'selected')),
    names_json    TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
