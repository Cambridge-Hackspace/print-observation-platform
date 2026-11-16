-- What POP has decided about each printer BAM! has (#23). A printer in the
-- BAM! category with no row here is offered to admins; nothing is added
-- without one. A retired printer keeps its row, and its history.
CREATE TABLE tower_printers (
    bam_id     TEXT PRIMARY KEY NOT NULL,
    state      TEXT NOT NULL CHECK (state IN ('added', 'declined', 'retired')),
    -- Its name when last decided, for a history that outlives BAM!'s.
    name       TEXT NOT NULL,
    decided_at INTEGER NOT NULL
) STRICT;
