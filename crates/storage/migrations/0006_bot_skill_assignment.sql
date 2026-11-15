CREATE TABLE bot_skill_assignment (
    bot_id        TEXT PRIMARY KEY REFERENCES bots(id),
    mode          TEXT NOT NULL CHECK (mode IN ('all', 'selected')),
    names_json    TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    version       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE bot_skill_assignment_state (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    generation INTEGER NOT NULL
);

INSERT INTO bot_skill_assignment_state (id, generation) VALUES (1, 1);

CREATE TRIGGER bot_skill_assignment_ai AFTER INSERT ON bot_skill_assignment BEGIN
    UPDATE bot_skill_assignment_state SET generation = generation + 1 WHERE id = 1;
END;
CREATE TRIGGER bot_skill_assignment_au AFTER UPDATE ON bot_skill_assignment BEGIN
    UPDATE bot_skill_assignment_state SET generation = generation + 1 WHERE id = 1;
END;
CREATE TRIGGER bot_skill_assignment_ad AFTER DELETE ON bot_skill_assignment BEGIN
    UPDATE bot_skill_assignment_state SET generation = generation + 1 WHERE id = 1;
END;
