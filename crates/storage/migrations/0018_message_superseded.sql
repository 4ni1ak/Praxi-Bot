-- A message the owner replaced (edit & resend) or whose answer was regenerated.
-- It stays in storage as audit data but is no longer part of the model context.
ALTER TABLE messages ADD COLUMN superseded INTEGER NOT NULL DEFAULT 0;
