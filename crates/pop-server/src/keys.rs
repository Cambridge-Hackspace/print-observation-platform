//! Personal API keys (#13): a member's keys for their slicer.
//!
//! A key is `pop_` and 32 random bytes, shown once when it is made and kept
//! only as its SHA-256, with its first characters so its owner can tell
//! their keys apart. It speaks for its owner on the routes marked for keys
//! (`Need::Key`, crate::access) and on no other: a request carrying a key
//! gets from every other route exactly what it would without one. It can
//! never control a printer: such a route needs at least printer-class
//! (`access::violations`), which no key gives.
//!
//! Its owner lists and revokes their own keys. A revoked key is kept,
//! refused, so they can see when it stopped working; a deactivated
//! account's keys are refused while it is.

use axum::{
    Extension, Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sqlx::SqlitePool;

use crate::{
    AppState,
    api::{ApiError, ApiKey, AuditKind, CreatedApiKey, NewApiKey, User},
    audit::{self, Actor, Event},
    users::{now, token_hash},
};

/// What every key starts with, so one pasted somewhere it should not be is
/// recognisable as POP's.
pub const PREFIX: &str = "pop_";

/// The most live keys one person may have.
pub const MAX_LIVE: i64 = 20;

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

/// The account a key speaks for, if it is live: not revoked, its owner
/// active. Marks it used.
pub async fn user_for(db: &SqlitePool, key: &str) -> Result<Option<User>, sqlx::Error> {
    if !key.starts_with(PREFIX) {
        return Ok(None);
    }
    let hash = token_hash(key);
    let row: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT u.id, u.username, u.name, u.role
           FROM api_keys k JOIN users u ON u.id = k.user_id
          WHERE k.token_hash = ? AND k.revoked_at IS NULL AND u.active = 1",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;
    let Some((id, username, name, role)) = row else {
        return Ok(None);
    };
    sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE token_hash = ?")
        .bind(now())
        .bind(&hash)
        .execute(db)
        .await?;
    Ok(Some(User {
        id,
        username,
        name,
        role: crate::users::role_from(&role),
    }))
}

/// A key's row: id, name, prefix, made, last used, revoked.
type KeyRow = (i64, String, String, i64, Option<i64>, Option<i64>);

/// `GET /api/keys`: the signed-in person's keys, newest first. Never a key
/// itself.
pub async fn list(State(state): State<AppState>, Extension(me): Extension<User>) -> Response {
    let rows: Result<Vec<KeyRow>, _> = sqlx::query_as(
        "SELECT id, name, prefix, created_at, last_used_at, revoked_at
           FROM api_keys WHERE user_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(me.id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(
                    |(id, name, prefix, created_at, last_used_at, revoked_at)| ApiKey {
                        id,
                        name,
                        prefix,
                        created_at,
                        last_used_at,
                        revoked_at,
                    },
                )
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `POST /api/keys`: a new key for the signed-in person, named for the
/// slicer it goes in. The answer is the only time the key is shown.
pub async fn create(
    State(state): State<AppState>,
    Extension(me): Extension<User>,
    body: Result<Json<NewApiKey>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return error(
            StatusCode::BAD_REQUEST,
            "name: what the key is for, such as \"PrusaSlicer on my laptop\", 1 to 64 characters",
        );
    }
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    let secret = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(b));
    let prefix: String = secret.chars().take(PREFIX.len() + 6).collect();
    let created_at = now();
    let done = async {
        let mut tx = state.db.begin().await?;
        let live: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM api_keys WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(me.id)
        .fetch_one(&mut *tx)
        .await?;
        if live >= MAX_LIVE {
            return Ok(None);
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO api_keys (user_id, name, prefix, token_hash, created_at)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(me.id)
        .bind(name)
        .bind(&prefix)
        .bind(token_hash(&secret))
        .bind(created_at)
        .fetch_one(&mut *tx)
        .await?;
        audit::record(
            &mut tx,
            &Actor::user(me.id, &me.name),
            Event {
                kind: AuditKind::ApiKeyCreated,
                printer: None,
                subject: Some((me.id, &me.name)),
                // Never the key.
                summary: format!("{} made the API key \"{name}\"", me.name),
            },
        )
        .await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(Some(id))
    }
    .await;
    match done {
        Ok(Some(id)) => (
            StatusCode::CREATED,
            Json(CreatedApiKey {
                key: ApiKey {
                    id,
                    name: name.to_owned(),
                    prefix,
                    created_at,
                    last_used_at: None,
                    revoked_at: None,
                },
                secret,
            }),
        )
            .into_response(),
        Ok(None) => error(
            StatusCode::CONFLICT,
            format!("you have {MAX_LIVE} keys already: revoke one you no longer use first"),
        ),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `DELETE /api/keys/{id}`: revoke one of the signed-in person's own keys.
/// Someone else's is not found, as one that does not exist.
pub async fn revoke(
    State(state): State<AppState>,
    Extension(me): Extension<User>,
    Path(id): Path<i64>,
) -> Response {
    let done = async {
        let mut tx = state.db.begin().await?;
        let name: Option<String> = sqlx::query_scalar(
            "UPDATE api_keys SET revoked_at = ?
              WHERE id = ? AND user_id = ? AND revoked_at IS NULL RETURNING name",
        )
        .bind(now())
        .bind(id)
        .bind(me.id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(name) = name else {
            return Ok(false);
        };
        audit::record(
            &mut tx,
            &Actor::user(me.id, &me.name),
            Event {
                kind: AuditKind::ApiKeyRevoked,
                printer: None,
                subject: Some((me.id, &me.name)),
                summary: format!("{} revoked the API key \"{name}\"", me.name),
            },
        )
        .await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(true)
    }
    .await;
    match done {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(
            StatusCode::NOT_FOUND,
            "you have no such key, or it is revoked already",
        ),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `GET /api/version`, as OctoPrint answers it: what a slicer asks first,
/// with the key, to test the connection (#37). It says the key works.
///
/// PrusaSlicer, SuperSlicer and OrcaSlicer refuse a host whose `text` does
/// not start "OctoPrint" (their `OctoPrint::validate_version_text`), and ask
/// this before every upload, so it does; POP's own name follows.
pub async fn version() -> Response {
    Json(serde_json::json!({
        "api": "0.1",
        "server": "1.10.0",
        "text": format!("OctoPrint 1.10.0 (POP {})", env!("CARGO_PKG_VERSION")),
    }))
    .into_response()
}
