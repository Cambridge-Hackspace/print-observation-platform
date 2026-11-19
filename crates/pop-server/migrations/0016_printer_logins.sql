-- A printer's own login (#29, #30): OctoPrint's API key, PrusaLink's user
-- and password or key, sealed as the other printer secrets are. SQLite
-- cannot change a CHECK in place, so the table is made again and its rows
-- copied over.
CREATE TABLE printer_secrets_new (
    bam_id     TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('access_code', 'camera_login', 'printer_login')),
    nonce      BLOB NOT NULL,
    sealed     BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (bam_id, kind)
) STRICT;
INSERT INTO printer_secrets_new (bam_id, kind, nonce, sealed, updated_at)
    SELECT bam_id, kind, nonce, sealed, updated_at FROM printer_secrets;
DROP TABLE printer_secrets;
ALTER TABLE printer_secrets_new RENAME TO printer_secrets;
