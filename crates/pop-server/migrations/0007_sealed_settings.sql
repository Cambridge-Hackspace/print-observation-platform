-- Tower-wide settings that are secrets (#68), such as the group's Discord
-- webhook: sealed like a printer's access code (#21), with the key bound in
-- as associated data, and never shown in full once saved.
CREATE TABLE sealed_settings (
    key        TEXT PRIMARY KEY NOT NULL,
    nonce      BLOB NOT NULL,
    sealed     BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
