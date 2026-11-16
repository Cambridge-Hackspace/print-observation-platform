-- Tower-wide settings, one row per key. The first table, and the one the
-- check defaults (off, warn or block; ADR 0001) will live in.
CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;
