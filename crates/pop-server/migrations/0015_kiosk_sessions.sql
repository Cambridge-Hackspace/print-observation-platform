-- Kiosk sessions (#12): a session started at an enrolled kiosk ends after
-- idle_secs with the screen left alone. NULL idle_secs is an ordinary session,
-- ending only at expires_at; last_used_at is when it was last used.
ALTER TABLE sessions ADD COLUMN idle_secs INTEGER;
ALTER TABLE sessions ADD COLUMN last_used_at INTEGER;
