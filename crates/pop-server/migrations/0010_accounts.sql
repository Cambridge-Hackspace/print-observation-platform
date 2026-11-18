-- Managing local users (#90): an account can be deactivated, which signs it
-- out and keeps it from signing in, without losing its history; and it can
-- hold local training, named as a printer's BAM! training field names it.
ALTER TABLE users ADD COLUMN active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1));

CREATE TABLE user_training (
    user_id  INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- "advanced": compared without regard to case, as BAM!'s field is typed.
    training TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (user_id, training)
) STRICT;
