-- The certificate each Bambu printer showed the first time POP connected
-- (#31): trusted on first use, and a different one later is refused. Kept
-- here so a restart does not trust a new certificate afresh.
CREATE TABLE bambu_pins (
    serial      TEXT PRIMARY KEY NOT NULL,
    fingerprint TEXT NOT NULL,
    pinned_at   INTEGER NOT NULL
) STRICT;
