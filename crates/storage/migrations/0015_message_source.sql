-- Authenticated origin at message creation: owner (local UI + owner key), local_api,
-- peer, telegram, channel, schedule, external_agent; empty/unknown rows never waive approval.
ALTER TABLE messages ADD COLUMN source TEXT NOT NULL DEFAULT '';
