-- Signing in through an OpenID Connect provider (#14, #16). An account is
-- linked to a provider's subject, never to an email or a username, which a
-- provider's members can change or share; the first sign-in with a new
-- subject makes a member account (cal, 2026-11-19).
CREATE TABLE user_identities (
    provider        TEXT NOT NULL,
    subject         TEXT NOT NULL,
    user_id         INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at      INTEGER NOT NULL,
    last_sign_in_at INTEGER NOT NULL,
    PRIMARY KEY (provider, subject)
) STRICT;
CREATE INDEX user_identities_user ON user_identities (user_id);

-- The provider whose configured roles made this account an admin, if one
-- did. Its sign-in may take back only what it gave: an admin made on the
-- desk is not demoted for lacking the provider's role.
ALTER TABLE users ADD COLUMN admin_from TEXT;
