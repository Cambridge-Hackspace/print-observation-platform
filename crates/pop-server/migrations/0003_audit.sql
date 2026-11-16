-- Who did what, and when (#8). Every feature that changes something writes
-- here, in the same transaction as the change.
CREATE TABLE audit (
    id           INTEGER PRIMARY KEY,
    at           INTEGER NOT NULL,
    -- Who acted. NULL is the command line, or no one signed in. The name is
    -- copied in, so the record still reads right after a rename or removal.
    actor_id     INTEGER REFERENCES users (id) ON DELETE SET NULL,
    actor_name   TEXT,
    kind         TEXT NOT NULL,
    -- What it concerned, when it was a printer or an account.
    printer      TEXT,
    subject_id   INTEGER REFERENCES users (id) ON DELETE SET NULL,
    subject_name TEXT,
    -- One line a person reads, written when it happened.
    summary      TEXT NOT NULL
) STRICT;

CREATE INDEX audit_by_time ON audit (at DESC, id DESC);
CREATE INDEX audit_by_actor ON audit (actor_id);
CREATE INDEX audit_by_subject ON audit (subject_id);
CREATE INDEX audit_by_printer ON audit (printer);
CREATE INDEX audit_by_kind ON audit (kind);
