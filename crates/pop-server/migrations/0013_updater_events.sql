-- What the updater did (#148), as POP took it from the updater's own record
-- (<data_dir>/updater/events.jsonl): each once, by its id, also recorded in
-- the audit log. The newest is what admins see as the last result.
CREATE TABLE updater_events (
    id      TEXT PRIMARY KEY,
    at      INTEGER NOT NULL,
    kind    TEXT NOT NULL,
    version TEXT,
    summary TEXT NOT NULL
) STRICT;
