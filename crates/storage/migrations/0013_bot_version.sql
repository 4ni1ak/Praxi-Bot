-- One revision per bot, including every profile/skill assignment writer.
ALTER TABLE bots ADD COLUMN version INTEGER NOT NULL DEFAULT 1 CHECK (typeof(version) = 'integer' AND version >= 1);

-- Also cover SQL writers that do not explicitly increment the revision.
CREATE TRIGGER bots_profile_revision AFTER UPDATE OF name, config_json ON bots
WHEN NEW.version = OLD.version BEGIN
    UPDATE bots SET version = OLD.version + 1 WHERE id = NEW.id;
END;
CREATE TRIGGER bot_skills_revision_ai AFTER INSERT ON bot_skill_assignment BEGIN
    UPDATE bots SET version = version + 1 WHERE id = NEW.bot_id;
END;
CREATE TRIGGER bot_skills_revision_au AFTER UPDATE ON bot_skill_assignment BEGIN
    UPDATE bots SET version = version + 1 WHERE id = NEW.bot_id OR id = OLD.bot_id;
END;
CREATE TRIGGER bot_skills_revision_ad AFTER DELETE ON bot_skill_assignment BEGIN
    UPDATE bots SET version = version + 1 WHERE id = OLD.bot_id;
END;
