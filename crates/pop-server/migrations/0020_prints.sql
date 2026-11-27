-- Prints waiting to be sent (#40): each member's uploads, kept as files under
-- files/prints/ (named by id), with what the slicer wrote into them.
--
-- A print waits until it is sent, its owner or an admin deletes it, or it
-- expires. Deleted, it is gone, row and file. Expired, its file is deleted and
-- its row stays, as the notice that tells its owner, until they dismiss it.
CREATE TABLE prints (
    -- 32 hex characters; the file's name.
    id          TEXT PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- As uploaded, for showing; never a path.
    filename    TEXT NOT NULL,
    format      TEXT NOT NULL CHECK (format IN ('gcode', 'gcode.3mf')),
    bytes       INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    -- JSON: the plates, as pop_gcode read them (api::PrintPlate).
    plates      TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    -- When the sweep deleted its file. NULL while it waits.
    expired_at  INTEGER
) STRICT;
CREATE INDEX prints_user ON prints (user_id, expired_at);
CREATE INDEX prints_waiting ON prints (expires_at) WHERE expired_at IS NULL;
