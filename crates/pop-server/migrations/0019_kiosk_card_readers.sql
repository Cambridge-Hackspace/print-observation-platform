-- A kiosk's card reader (#19): the CS:S resource its reader is, as the reader
-- names it. CS:S's answers to taps there sign members in at this kiosk, and
-- at no other: one reader, one kiosk, while the kiosk is enrolled.
ALTER TABLE kiosks ADD COLUMN card_resource TEXT;
CREATE UNIQUE INDEX kiosks_card_resource ON kiosks (card_resource)
    WHERE card_resource IS NOT NULL AND revoked_at IS NULL;
