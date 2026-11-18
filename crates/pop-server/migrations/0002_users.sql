-- Local accounts and their sign-ins (#11). Two roles: member and admin.
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    -- Unique without regard to case: "Ada" and "ada" are one person.
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    name          TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('member', 'admin')),
    -- An argon2id PHC string; the password itself is never stored.
    password_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL
) STRICT;

-- One row per signed-in browser. The token itself lives only in the
-- browser's cookie; this holds its SHA-256, so a copy of the database signs
-- nobody in.
CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX sessions_by_user ON sessions (user_id);
