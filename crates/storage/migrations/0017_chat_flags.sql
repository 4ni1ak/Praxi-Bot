-- Owner-controlled chat organisation: pinned chats sort first, archived chats are hidden from
-- the default list. Neither changes the conversation itself.
ALTER TABLE chats ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
