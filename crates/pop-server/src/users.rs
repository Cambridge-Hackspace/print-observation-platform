//! Local accounts (#11): two roles, member and admin; passwords hashed with
//! argon2id; sign-ins as sessions.
//!
//! A session is a random token in the browser's cookie. The database holds
//! only the token's SHA-256, so a copy of the database signs nobody in.

use std::{
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::{
    api::{AuditKind, Role, User},
    audit::{self, Actor, Event},
};

/// How long a sign-in lasts. Kiosk sessions, with their own timeout, are #12.
pub const SESSION_SECS: i64 = 30 * 24 * 60 * 60;

#[derive(Debug, PartialEq, Eq)]
pub enum UserError {
    /// A value that is not allowed, naming the field and why.
    Invalid {
        field: &'static str,
        reason: String,
    },
    UsernameTaken,
    NotFound,
    /// An admin cannot be made a member if no other admin would remain.
    LastAdmin,
    Database(String),
}

impl From<sqlx::Error> for UserError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e.to_string())
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What a new account needs.
pub struct NewAccount<'a> {
    pub username: &'a str,
    pub name: &'a str,
    pub role: Role,
    pub password: &'a str,
}

fn invalid(field: &'static str, reason: impl Into<String>) -> UserError {
    UserError::Invalid {
        field,
        reason: reason.into(),
    }
}

pub fn check_username(u: &str) -> Result<(), UserError> {
    if u.is_empty() || u.len() > 64 {
        return Err(invalid("username", "must be 1 to 64 characters"));
    }
    if !u
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(invalid(
            "username",
            "may use letters, digits, dots, dashes and underscores",
        ));
    }
    Ok(())
}

pub fn check_name(n: &str) -> Result<(), UserError> {
    let n = n.trim();
    if n.is_empty() || n.chars().count() > 100 {
        return Err(invalid("name", "must be 1 to 100 characters"));
    }
    Ok(())
}

pub fn check_password(p: &str) -> Result<(), UserError> {
    let n = p.chars().count();
    if n < 8 {
        return Err(invalid("password", "must be at least 8 characters"));
    }
    // Hashing is the cost an attacker pays and so do we: an unbounded
    // password would let one request spend it without limit.
    if p.len() > 1024 {
        return Err(invalid("password", "must be at most 1024 bytes"));
    }
    Ok(())
}

pub fn hash_password(p: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(p.as_bytes(), &salt)
        .map(|h| h.to_string())
        // Hashing fails only on parameters or salt, both fixed here.
        .expect("argon2 hashing with default parameters")
}

fn verify(p: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .map(|h| Argon2::default().verify_password(p.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// A hash no password matches, checked when the username does not exist, so
/// a wrong username takes as long to refuse as a wrong password and the
/// timing does not say which usernames exist.
fn decoy() -> &'static str {
    static DECOY: OnceLock<String> = OnceLock::new();
    DECOY.get_or_init(|| {
        let mut b = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut b);
        hash_password(&URL_SAFE_NO_PAD.encode(b))
    })
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    username: String,
    name: String,
    role: String,
}

impl From<Row> for User {
    fn from(r: Row) -> Self {
        User {
            id: r.id,
            username: r.username,
            name: r.name,
            role: if r.role == "admin" {
                Role::Admin
            } else {
                Role::Member
            },
        }
    }
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::Admin => "admin",
        Role::Member => "member",
    }
}

pub async fn create(db: &SqlitePool, a: NewAccount<'_>, by: &Actor) -> Result<User, UserError> {
    check_username(a.username)?;
    check_name(a.name)?;
    check_password(a.password)?;
    let hash = hash_password(a.password);
    let mut tx = db.begin().await?;
    let r = sqlx::query_as::<_, Row>(
        "INSERT INTO users (username, name, role, password_hash, created_at)
         VALUES (?, ?, ?, ?, ?)
         RETURNING id, username, name, role",
    )
    .bind(a.username)
    .bind(a.name.trim())
    .bind(role_str(a.role))
    .bind(hash)
    .bind(now())
    .fetch_one(&mut *tx)
    .await;
    let user: User = match r {
        Ok(row) => row.into(),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(UserError::UsernameTaken);
        }
        Err(e) => return Err(e.into()),
    };
    audit::record(
        &mut tx,
        by,
        Event {
            kind: AuditKind::UserCreated,
            printer: None,
            subject: Some((user.id, &user.name)),
            summary: format!(
                "{} created {}'s account ({}, {})",
                by_name(by),
                user.name,
                user.username,
                role_str(user.role)
            ),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(user)
}

/// How an actor is named in a summary.
fn by_name(by: &Actor) -> &str {
    by.name.as_deref().unwrap_or("someone")
}

pub async fn list(db: &SqlitePool) -> Result<Vec<User>, UserError> {
    Ok(sqlx::query_as::<_, Row>(
        "SELECT id, username, name, role FROM users ORDER BY username COLLATE NOCASE",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

pub async fn get(db: &SqlitePool, id: i64) -> Result<User, UserError> {
    sqlx::query_as::<_, Row>("SELECT id, username, name, role FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .map(Into::into)
        .ok_or(UserError::NotFound)
}

/// The account `username` and `password` sign in to, if they match one.
pub async fn authenticate(
    db: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<Option<User>, UserError> {
    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(db)
            .await?;
    let (id, phc) = match &row {
        Some((id, phc)) => (Some(*id), phc.as_str()),
        None => (None, decoy()),
    };
    // Hash before deciding, whichever way it goes.
    let ok = verify(password, phc);
    match (id, ok) {
        (Some(id), true) => Ok(Some(get(db, id).await?)),
        _ => Ok(None),
    }
}

/// Set a new password, and end every session the account has, so whoever
/// knew the old one is signed out.
pub async fn reset_password(
    db: &SqlitePool,
    id: i64,
    password: &str,
    by: &Actor,
) -> Result<(), UserError> {
    check_password(password)?;
    let hash = hash_password(password);
    let mut tx = db.begin().await?;
    let name: Option<String> =
        sqlx::query_scalar("UPDATE users SET password_hash = ? WHERE id = ? RETURNING name")
            .bind(hash)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let name = name.ok_or(UserError::NotFound)?;
    let ended = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    audit::record(
        &mut tx,
        by,
        Event {
            kind: AuditKind::PasswordReset,
            printer: None,
            subject: Some((id, &name)),
            summary: format!(
                "{} reset {name}'s password, ending {ended} sign-in(s)",
                by_name(by)
            ),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn set_role(db: &SqlitePool, id: i64, role: Role, by: &Actor) -> Result<User, UserError> {
    let mut tx = db.begin().await?;
    let current: Option<(String, String)> =
        sqlx::query_as("SELECT role, name FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (current, name) = current.ok_or(UserError::NotFound)?;
    if current == role_str(role) {
        tx.commit().await?;
        return get(db, id).await;
    }
    if current == "admin" && role == Role::Member {
        let admins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin'")
            .fetch_one(&mut *tx)
            .await?;
        if admins <= 1 {
            return Err(UserError::LastAdmin);
        }
    }
    sqlx::query("UPDATE users SET role = ? WHERE id = ?")
        .bind(role_str(role))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        by,
        Event {
            kind: AuditKind::RoleChanged,
            printer: None,
            subject: Some((id, &name)),
            summary: format!(
                "{} made {name} {} (was {current})",
                by_name(by),
                match role {
                    Role::Admin => "an admin",
                    Role::Member => "a member",
                }
            ),
        },
    )
    .await?;
    tx.commit().await?;
    get(db, id).await
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Start a session for `user`, returning the token for its cookie.
pub async fn start_session(db: &SqlitePool, user: &User) -> Result<String, UserError> {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    let token = URL_SAFE_NO_PAD.encode(b);
    let t = now();
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(user.id)
    .bind(t)
    .bind(t + SESSION_SECS)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        &Actor::user(user.id, &user.name),
        Event {
            kind: AuditKind::SignedIn,
            printer: None,
            subject: Some((user.id, &user.name)),
            summary: format!("{} signed in", user.name),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(token)
}

/// A sign-in that failed, for the username given. Recorded so an admin can
/// see someone guessing; the password tried is never recorded.
pub async fn sign_in_failed(db: &SqlitePool, username: &str) -> Result<(), UserError> {
    let mut conn = db.acquire().await?;
    // At most 64 characters of what was typed, so the log cannot be stuffed.
    let shown: String = username.chars().take(64).collect();
    audit::record(
        &mut conn,
        &Actor::default(),
        Event {
            kind: AuditKind::SignInFailed,
            printer: None,
            subject: None,
            summary: format!("a sign-in as {shown:?} failed"),
        },
    )
    .await?;
    Ok(())
}

/// The account a session token belongs to, if the session is live.
pub async fn session_user(db: &SqlitePool, token: &str) -> Result<Option<User>, UserError> {
    Ok(sqlx::query_as::<_, Row>(
        "SELECT u.id, u.username, u.name, u.role
           FROM sessions s JOIN users u ON u.id = s.user_id
          WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash(token))
    .bind(now())
    .fetch_optional(db)
    .await?
    .map(Into::into))
}

/// End the session `token`, signing out `user`.
pub async fn end_session(db: &SqlitePool, token: &str, user: &User) -> Result<(), UserError> {
    let mut tx = db.begin().await?;
    let n = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash(token))
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n > 0 {
        audit::record(
            &mut tx,
            &Actor::user(user.id, &user.name),
            Event {
                kind: AuditKind::SignedOut,
                printer: None,
                subject: Some((user.id, &user.name)),
                summary: format!("{} signed out", user.name),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
