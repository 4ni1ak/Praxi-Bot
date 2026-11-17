-- A schedule that moved to a server is FROZEN on this computer: it never fires and no ordinary
-- update can enable it again. The guard lives in the database so that every mutation path
-- (HTTP update, bot tools, CLI, import) is covered; only `ScheduleStore::unfreeze` clears it.
ALTER TABLE schedules ADD COLUMN frozen_for TEXT;
CREATE TRIGGER schedules_frozen_cannot_enable
BEFORE UPDATE OF enabled ON schedules
WHEN NEW.enabled <> 0 AND OLD.frozen_for IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'schedule is frozen: it moved to a server');
END;
