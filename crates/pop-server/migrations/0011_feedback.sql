-- Feedback from any screen (#96), kept until GitHub has it as an issue, so a
-- report made while GitHub or the network is down is sent once it is back.
CREATE TABLE feedback_reports (
    id          TEXT PRIMARY KEY,
    created_at  INTEGER NOT NULL,
    account_id  INTEGER NOT NULL REFERENCES users (id),
    kind        TEXT NOT NULL CHECK (kind IN ('bug', 'suggestion', 'question')),
    title       TEXT NOT NULL,
    -- The issue's body, composed when it was made: description, context.
    body        TEXT NOT NULL,
    screenshot  BLOB NOT NULL,
    state       TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'sent', 'failed')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    -- When to try sending it next, in milliseconds since 1970.
    next_try_at INTEGER NOT NULL,
    last_error  TEXT,
    issue_url   TEXT
) STRICT;

CREATE INDEX feedback_reports_due ON feedback_reports (state, next_try_at);
CREATE INDEX feedback_reports_by_account ON feedback_reports (account_id, created_at);
