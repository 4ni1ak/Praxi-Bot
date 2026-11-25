-- Optional catch-up window: after downtime, a missed occurrence older than this
-- many seconds is not run late; it is recorded as `missed` and shown. NULL keeps
-- the previous behaviour (no limit), so existing schedules do not change.
ALTER TABLE schedules ADD COLUMN catchup_window_secs INTEGER CHECK (catchup_window_secs IS NULL OR catchup_window_secs > 0);
