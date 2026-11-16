-- Secrets POP keeps for printers, never BAM! (#21): a Bambu printer's LAN
-- access code. Sealed with ChaCha20-Poly1305 under [secrets] key, the
-- printer's BAM! id bound in as associated data.
CREATE TABLE printer_secrets (
    bam_id     TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('access_code')),
    nonce      BLOB NOT NULL,
    sealed     BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (bam_id, kind)
) STRICT;
