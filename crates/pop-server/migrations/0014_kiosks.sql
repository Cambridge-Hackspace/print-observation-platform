-- Kiosks (#12): the print stations an admin has enrolled. A kiosk holds a
-- device credential, a random token in an HttpOnly cookie, kept here only as
-- its SHA-256; it checks in with it, and a network is the space's only while
-- an enrolled kiosk has checked in from it lately (crate::access).
CREATE TABLE kiosks (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    enrolled_by  INTEGER REFERENCES users (id) ON DELETE SET NULL,
    enrolled_at  INTEGER NOT NULL,
    revoked_at   INTEGER,
    last_seen_at INTEGER,
    last_address TEXT
) STRICT;
