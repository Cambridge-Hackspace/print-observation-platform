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
    api::{Account, AuditKind, Role, User},
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
            role: role_from(&r.role),
        }
    }
}

/// A role as the database keeps it; anything but "admin" is a member.
pub(crate) fn role_from(s: &str) -> Role {
    if s == "admin" {
        Role::Admin
    } else {
        Role::Member
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
    let row: Option<(i64, String, bool)> =
        sqlx::query_as("SELECT id, password_hash, active FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(db)
            .await?;
    let (id, phc, active) = match &row {
        Some((id, phc, active)) => (Some(*id), phc.as_str(), *active),
        None => (None, decoy(), false),
    };
    // Hash before deciding, whichever way it goes, so a deactivated account
    // answers in the same time as a wrong password.
    let ok = verify(password, phc);
    match (id, ok, active) {
        (Some(id), true, true) => Ok(Some(get(db, id).await?)),
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
    if current == "admin" && role == Role::Member && active_admins_besides(&mut tx, id).await? == 0
    {
        return Err(UserError::LastAdmin);
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

/// How many active admins there are other than `id`: a change that would
/// leave none is refused, since nobody could then manage POP.
async fn active_admins_besides(
    conn: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<i64, UserError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1 AND id != ?",
    )
    .bind(id)
    .fetch_one(conn)
    .await?)
}

/// The most training an account can hold, and the longest name for one.
const MOST_TRAINING: usize = 32;
const LONGEST_TRAINING: usize = 40;

/// Every account, as an admin manages it (#90).
pub async fn accounts(db: &SqlitePool) -> Result<Vec<Account>, UserError> {
    let rows: Vec<(i64, String, String, String, bool)> = sqlx::query_as(
        "SELECT id, username, name, role, active FROM users ORDER BY username COLLATE NOCASE",
    )
    .fetch_all(db)
    .await?;
    let held: Vec<(i64, String)> = sqlx::query_as(
        "SELECT user_id, training FROM user_training ORDER BY training COLLATE NOCASE",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, username, name, role, active)| Account {
            id,
            username,
            name,
            role: if role == "admin" {
                Role::Admin
            } else {
                Role::Member
            },
            active,
            training: held
                .iter()
                .filter(|(u, _)| *u == id)
                .map(|(_, t)| t.clone())
                .collect(),
        })
        .collect())
}

/// One account, as an admin manages it.
pub async fn account(db: &SqlitePool, id: i64) -> Result<Account, UserError> {
    accounts(db)
        .await?
        .into_iter()
        .find(|a| a.id == id)
        .ok_or(UserError::NotFound)
}

/// Rename an account. Its username, which it signs in with, stays.
pub async fn set_name(
    db: &SqlitePool,
    id: i64,
    name: &str,
    by: &Actor,
) -> Result<Account, UserError> {
    check_name(name)?;
    let name = name.trim();
    let mut tx = db.begin().await?;
    let was: Option<String> = sqlx::query_scalar("SELECT name FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let was = was.ok_or(UserError::NotFound)?;
    if was != name {
        sqlx::query("UPDATE users SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        audit::record(
            &mut tx,
            by,
            Event {
                kind: AuditKind::NameChanged,
                printer: None,
                subject: Some((id, name)),
                summary: format!("{} renamed {was} to {name}", by_name(by)),
            },
        )
        .await?;
    }
    tx.commit().await?;
    account(db, id).await
}

/// Deactivate an account, ending every sign-in it has, or reactivate it. The
/// last active admin cannot be deactivated.
pub async fn set_active(
    db: &SqlitePool,
    id: i64,
    active: bool,
    by: &Actor,
) -> Result<Account, UserError> {
    let mut tx = db.begin().await?;
    let current: Option<(String, String, bool)> =
        sqlx::query_as("SELECT name, role, active FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (name, role, was) = current.ok_or(UserError::NotFound)?;
    if was != active {
        if !active && role == "admin" && active_admins_besides(&mut tx, id).await? == 0 {
            return Err(UserError::LastAdmin);
        }
        sqlx::query("UPDATE users SET active = ? WHERE id = ?")
            .bind(active)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let summary = if active {
            format!("{} reactivated {name}'s account", by_name(by))
        } else {
            let ended = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            format!(
                "{} deactivated {name}'s account, ending {ended} sign-in(s)",
                by_name(by)
            )
        };
        audit::record(
            &mut tx,
            by,
            Event {
                kind: if active {
                    AuditKind::AccountReactivated
                } else {
                    AuditKind::AccountDeactivated
                },
                printer: None,
                subject: Some((id, &name)),
                summary,
            },
        )
        .await?;
    }
    tx.commit().await?;
    account(db, id).await
}

/// Set every training an account holds. Each is trimmed, and one named twice
/// (in any case) is held once.
pub async fn set_training(
    db: &SqlitePool,
    id: i64,
    training: &[String],
    by: &Actor,
) -> Result<Account, UserError> {
    let mut wanted: Vec<String> = Vec::new();
    for t in training {
        let t = t.trim();
        if t.is_empty() || t.chars().count() > LONGEST_TRAINING || t.chars().any(char::is_control) {
            return Err(UserError::Invalid {
                field: "training",
                reason: format!(
                    "each is 1 to {LONGEST_TRAINING} characters, as the printer's BAM! training field names it"
                ),
            });
        }
        if !wanted.iter().any(|w| w.eq_ignore_ascii_case(t)) {
            wanted.push(t.to_owned());
        }
    }
    if wanted.len() > MOST_TRAINING {
        return Err(UserError::Invalid {
            field: "training",
            reason: format!("at most {MOST_TRAINING}"),
        });
    }
    wanted.sort_by_key(|t| t.to_lowercase());
    let before = account(db, id).await?;
    let same = before.training.len() == wanted.len()
        && before
            .training
            .iter()
            .zip(&wanted)
            .all(|(a, b)| a.eq_ignore_ascii_case(b));
    if same {
        return Ok(before);
    }
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM user_training WHERE user_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for t in &wanted {
        sqlx::query("INSERT INTO user_training (user_id, training) VALUES (?, ?)")
            .bind(id)
            .bind(t)
            .execute(&mut *tx)
            .await?;
    }
    let shown = |v: &[String]| {
        if v.is_empty() {
            "none".to_owned()
        } else {
            v.join(", ")
        }
    };
    audit::record(
        &mut tx,
        by,
        Event {
            kind: AuditKind::TrainingChanged,
            printer: None,
            subject: Some((id, &before.name)),
            summary: format!(
                "{} set {}'s training to {} (was {})",
                by_name(by),
                before.name,
                shown(&wanted),
                shown(&before.training)
            ),
        },
    )
    .await?;
    tx.commit().await?;
    account(db, id).await
}

pub(crate) fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Start a session for `user`, returning the token for its cookie.
/// `outside`: the sign-in came from outside the space, which the audit log
/// says (#12).
pub async fn start_session(
    db: &SqlitePool,
    user: &User,
    outside: bool,
) -> Result<String, UserError> {
    start_session_with(db, user, outside, None).await
}

/// The same, as a kiosk session when `idle_secs` is given: it ends after
/// that long unused, whatever its expiry (#12).
pub async fn start_session_with(
    db: &SqlitePool,
    user: &User,
    outside: bool,
    idle_secs: Option<u32>,
) -> Result<String, UserError> {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    let token = URL_SAFE_NO_PAD.encode(b);
    let t = now();
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, idle_secs, last_used_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(user.id)
    .bind(t)
    .bind(t + SESSION_SECS)
    .bind(idle_secs.map(i64::from))
    .bind(t)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        &Actor::user(user.id, &user.name),
        Event {
            kind: AuditKind::SignedIn,
            printer: None,
            subject: Some((user.id, &user.name)),
            summary: match (idle_secs.is_some(), outside) {
                (true, _) => format!("{} signed in at the kiosk", user.name),
                (false, true) => format!("{} signed in from outside the space", user.name),
                (false, false) => format!("{} signed in", user.name),
            },
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
    let t = now();
    let user: Option<User> = sqlx::query_as::<_, Row>(
        "SELECT u.id, u.username, u.name, u.role
           FROM sessions s JOIN users u ON u.id = s.user_id
          WHERE s.token_hash = ? AND s.expires_at > ? AND u.active = 1
            AND (s.idle_secs IS NULL OR s.last_used_at + s.idle_secs > ?)",
    )
    .bind(token_hash(token))
    .bind(t)
    .bind(t)
    .fetch_optional(db)
    .await?
    .map(Into::into);
    if user.is_some() {
        // Used now: a kiosk session's idle time starts again (#12). An
        // ordinary one is not written to on every request.
        sqlx::query(
            "UPDATE sessions SET last_used_at = ? WHERE token_hash = ? AND idle_secs IS NOT NULL",
        )
        .bind(t)
        .bind(token_hash(token))
        .execute(db)
        .await?;
    }
    Ok(user)
}

/// A live session's idle limit, if it is a kiosk session (#12).
pub async fn session_idle_secs(db: &SqlitePool, token: &str) -> Result<Option<u32>, UserError> {
    let v: Option<Option<i64>> =
        sqlx::query_scalar("SELECT idle_secs FROM sessions WHERE token_hash = ?")
            .bind(token_hash(token))
            .fetch_optional(db)
            .await?;
    Ok(v.flatten().and_then(|s| u32::try_from(s).ok()))
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
