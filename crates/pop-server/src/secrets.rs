//! Secrets POP keeps (#21), such as a Bambu printer's LAN access code: never
//! in BAM!, never in plain text.
//!
//! Each is sealed with ChaCha20-Poly1305 under the configured key
//! ([secrets] key, or POP_SECRETS_KEY), with a fresh random nonce, and the
//! printer's BAM! id and the secret's kind bound in as associated data: a
//! sealed code copied to another printer's row does not open. Without a key,
//! POP refuses to keep a secret, rather than keep it in plain text.

use base64::Engine;
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit,
    aead::{Aead, AeadCore, OsRng, Payload},
};
use sqlx::SqlitePool;

use crate::users::now;

#[derive(Debug, PartialEq, Eq)]
pub enum SecretError {
    /// No [secrets] key is configured.
    NoKey,
    /// A sealed secret would not open: the key changed, or the row was moved
    /// or tampered with.
    WillNotOpen,
    Database(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoKey => write!(
                f,
                "POP has no secrets key: set POP_SECRETS_KEY, then enter it again"
            ),
            Self::WillNotOpen => write!(
                f,
                "a stored secret will not open: the key changed, so enter it again"
            ),
            Self::Database(e) => write!(f, "{e}"),
        }
    }
}

impl From<sqlx::Error> for SecretError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e.to_string())
    }
}

/// Seals and opens secrets with the configured key.
#[derive(Clone)]
pub struct Secrets {
    cipher: Option<ChaCha20Poly1305>,
}

const ACCESS_CODE: &str = "access_code";
const SETTING: &str = "setting";

impl Secrets {
    /// From [secrets] key, already checked to be 32 bytes in base64; empty
    /// means no key.
    pub fn new(key_b64: &str) -> Self {
        let cipher = (!key_b64.is_empty())
            .then(|| {
                base64::engine::general_purpose::STANDARD
                    .decode(key_b64)
                    .ok()
            })
            .flatten()
            .and_then(|k| ChaCha20Poly1305::new_from_slice(&k).ok());
        Self { cipher }
    }

    pub fn none() -> Self {
        Self { cipher: None }
    }

    pub fn has_key(&self) -> bool {
        self.cipher.is_some()
    }

    fn aad(bam_id: &str, kind: &str) -> Vec<u8> {
        format!("pop:{kind}:{bam_id}").into_bytes()
    }

    /// Keep a printer's access code.
    pub async fn set_access_code(
        &self,
        db: &SqlitePool,
        bam_id: &str,
        code: &str,
    ) -> Result<(), SecretError> {
        let cipher = self.cipher.as_ref().ok_or(SecretError::NoKey)?;
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let sealed = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: code.as_bytes(),
                    aad: &Self::aad(bam_id, ACCESS_CODE),
                },
            )
            .map_err(|_| SecretError::WillNotOpen)?;
        sqlx::query(
            "INSERT INTO printer_secrets (bam_id, kind, nonce, sealed, updated_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (bam_id, kind) DO UPDATE SET nonce = excluded.nonce, sealed = excluded.sealed,
                                                   updated_at = excluded.updated_at",
        )
        .bind(bam_id)
        .bind(ACCESS_CODE)
        .bind(nonce.as_slice())
        .bind(sealed)
        .bind(now())
        .execute(db)
        .await?;
        Ok(())
    }

    /// A printer's access code, if POP holds one.
    pub async fn access_code(
        &self,
        db: &SqlitePool,
        bam_id: &str,
    ) -> Result<Option<String>, SecretError> {
        let row: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT nonce, sealed FROM printer_secrets WHERE bam_id = ? AND kind = ?",
        )
        .bind(bam_id)
        .bind(ACCESS_CODE)
        .fetch_optional(db)
        .await?;
        let Some((nonce, sealed)) = row else {
            return Ok(None);
        };
        let cipher = self.cipher.as_ref().ok_or(SecretError::NoKey)?;
        if nonce.len() != 12 {
            return Err(SecretError::WillNotOpen);
        }
        let plain = cipher
            .decrypt(
                chacha20poly1305::Nonce::from_slice(&nonce),
                Payload {
                    msg: &sealed,
                    aad: &Self::aad(bam_id, ACCESS_CODE),
                },
            )
            .map_err(|_| SecretError::WillNotOpen)?;
        String::from_utf8(plain)
            .map(Some)
            .map_err(|_| SecretError::WillNotOpen)
    }

    /// Keep a tower-wide secret setting (#68), replacing any before it.
    pub async fn set_setting(
        &self,
        db: &SqlitePool,
        key: &str,
        value: &str,
    ) -> Result<(), SecretError> {
        let cipher = self.cipher.as_ref().ok_or(SecretError::NoKey)?;
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let sealed = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: value.as_bytes(),
                    aad: &Self::aad(key, SETTING),
                },
            )
            .map_err(|_| SecretError::WillNotOpen)?;
        sqlx::query(
            "INSERT INTO sealed_settings (key, nonce, sealed, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (key) DO UPDATE SET nonce = excluded.nonce, sealed = excluded.sealed,
                                             updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(nonce.as_slice())
        .bind(sealed)
        .bind(now())
        .execute(db)
        .await?;
        Ok(())
    }

    /// A tower-wide secret setting, if one is kept.
    pub async fn setting(&self, db: &SqlitePool, key: &str) -> Result<Option<String>, SecretError> {
        let row: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT nonce, sealed FROM sealed_settings WHERE key = ?")
                .bind(key)
                .fetch_optional(db)
                .await?;
        let Some((nonce, sealed)) = row else {
            return Ok(None);
        };
        let cipher = self.cipher.as_ref().ok_or(SecretError::NoKey)?;
        if nonce.len() != 12 {
            return Err(SecretError::WillNotOpen);
        }
        let plain = cipher
            .decrypt(
                chacha20poly1305::Nonce::from_slice(&nonce),
                Payload {
                    msg: &sealed,
                    aad: &Self::aad(key, SETTING),
                },
            )
            .map_err(|_| SecretError::WillNotOpen)?;
        String::from_utf8(plain)
            .map(Some)
            .map_err(|_| SecretError::WillNotOpen)
    }

    /// Forget a tower-wide secret setting. True if there was one.
    pub async fn clear_setting(db: &SqlitePool, key: &str) -> Result<bool, SecretError> {
        let done = sqlx::query("DELETE FROM sealed_settings WHERE key = ?")
            .bind(key)
            .execute(db)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// The BAM! ids of every printer whose access code POP holds.
    pub async fn with_access_codes(db: &SqlitePool) -> Result<Vec<String>, SecretError> {
        Ok(
            sqlx::query_scalar("SELECT bam_id FROM printer_secrets WHERE kind = ?")
                .bind(ACCESS_CODE)
                .fetch_all(db)
                .await?,
        )
    }
}
