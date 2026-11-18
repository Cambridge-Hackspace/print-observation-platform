-- A camera's login, kept sealed in POP (#150) rather than in the camera's
-- address in BAM!: printer_secrets gains the kind camera_login. SQLite
-- cannot change a CHECK in place, so the table is made again and its rows
-- copied over.
CREATE TABLE printer_secrets_new (
    bam_id     TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('access_code', 'camera_login')),
    nonce      BLOB NOT NULL,
    sealed     BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (bam_id, kind)
) STRICT;
INSERT INTO printer_secrets_new (bam_id, kind, nonce, sealed, updated_at)
    SELECT bam_id, kind, nonce, sealed, updated_at FROM printer_secrets;
DROP TABLE printer_secrets;
ALTER TABLE printer_secrets_new RENAME TO printer_secrets;
