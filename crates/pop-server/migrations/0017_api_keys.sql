-- Personal API keys (#13): a member's keys for their slicer. A key is shown
-- once, when it is made, and kept here only as its SHA-256, with its first
-- characters so its owner can tell their keys apart. A revoked key is kept,
-- refused, so the owner can see when it stopped working.
CREATE TABLE api_keys (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    prefix       TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at   INTEGER
) STRICT;
CREATE INDEX api_keys_user ON api_keys (user_id);
