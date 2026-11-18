//! Kiosks (#12): the print stations, enrolled once by an admin, which confirm
//! from their own side that they are where they are (cal, on #12).
//!
//! Enrolling gives the kiosk's browser a device credential: a random token in
//! an HttpOnly cookie no script can read, kept by POP only as its SHA-256, and
//! revocable from the desk. The kiosk's page checks in with it every half
//! minute ([`checkin`]); POP keeps the address each check-in came FROM, as the
//! socket and a trusted proxy say, never anything the kiosk says about itself.
//! A network is the space's while a kiosk has checked in from it within
//! [`FRESH`] (crate::access::is_local).

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Extension, Json,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ipnet::IpNet;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::{
    AppState,
    api::{ApiError, AuditKind, Kiosk, NewKiosk, User},
    audit::{self, Actor, Event},
    users::now,
};

/// The device credential's cookie.
pub const KIOSK_COOKIE: &str = "pop_kiosk";

/// How long a kiosk's check-in vouches for its network. Four check-ins'
/// worth, so one dropped request does not end the space.
pub const FRESH: Duration = Duration::from_secs(120);

/// Where each enrolled kiosk last checked in from, and when, in memory: a
/// restart forgets, and the next check-in, within half a minute, remembers.
#[derive(Clone, Default)]
pub struct Sightings(Arc<Mutex<HashMap<i64, (IpAddr, Instant)>>>);

impl Sightings {
    /// Kiosk `id` checked in from `from`, now.
    pub fn saw(&self, id: i64, from: IpAddr) {
        self.saw_at(id, from, Instant::now());
    }

    /// The same, at `at`: for a test that needs a check-in of a given age.
    pub fn saw_at(&self, id: i64, from: IpAddr, at: Instant) {
        self.0.lock().unwrap().insert(id, (from, at));
    }

    /// Kiosk `id` is revoked: its last check-in vouches for nothing.
    pub fn forget(&self, id: i64) {
        self.0.lock().unwrap().remove(&id);
    }

    /// Whether some kiosk has checked in from `net` within [`FRESH`].
    pub fn seen_on(&self, net: &IpNet) -> bool {
        self.0
            .lock()
            .unwrap()
            .values()
            .any(|(ip, at)| net.contains(ip) && at.elapsed() <= FRESH)
    }
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn kiosk_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == KIOSK_COOKIE)
        .map(|(_, v)| v.to_owned())
}

/// The cookie that makes a browser a kiosk. HttpOnly: no script reads it.
/// SameSite=Strict, and sent only to the API: the check-in reads it, and so
/// does signing in, which at a kiosk starts a kiosk session (#12). Nothing
/// else gives it any weight.
fn cookie(token: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{KIOSK_COOKIE}={token}; Path=/api; HttpOnly; SameSite=Strict; Max-Age={max_age}"
    ))
    .expect("a cookie of base64url and fixed text is a valid header")
}

/// Ten years: a kiosk is enrolled until it is revoked, not until it expires.
const COOKIE_SECS: i64 = 10 * 365 * 24 * 3600;

/// `POST /api/admin/kiosks`: make THIS browser a kiosk, named.
pub async fn enroll(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    body: Result<Json<NewKiosk>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return error(
            StatusCode::BAD_REQUEST,
            "name: what to call this kiosk, 1 to 64 characters",
        );
    }
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    let token = URL_SAFE_NO_PAD.encode(b);
    let done = async {
        let mut tx = state.db.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kiosks (name, token_hash, enrolled_by, enrolled_at) VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(hash(&token))
        .bind(admin.id)
        .bind(now())
        .fetch_one(&mut *tx)
        .await?;
        audit::record(
            &mut tx,
            &Actor::user(admin.id, &admin.name),
            Event {
                kind: AuditKind::KioskEnrolled,
                printer: None,
                subject: None,
                summary: format!("{} enrolled the kiosk {name}", admin.name),
            },
        )
        .await?;
        tx.commit().await?;
        Ok::<i64, sqlx::Error>(id)
    }
    .await;
    match done {
        Ok(id) => {
            let mut res =
                (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response();
            res.headers_mut()
                .insert(header::SET_COOKIE, cookie(&token, COOKIE_SECS));
            res
        }
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// A kiosk as the table holds it: id, name, enrolled_at, revoked_at,
/// last_seen_at, last_address.
type KioskRow = (i64, String, i64, Option<i64>, Option<i64>, Option<String>);

/// `GET /api/admin/kiosks`: every kiosk ever enrolled, revoked ones included.
pub async fn list(State(state): State<AppState>) -> Response {
    let rows: Result<Vec<KioskRow>, _> =
        sqlx::query_as(
            "SELECT id, name, enrolled_at, revoked_at, last_seen_at, last_address FROM kiosks ORDER BY id",
        )
        .fetch_all(&state.db)
        .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(
                    |(id, name, enrolled_at, revoked_at, last_seen_at, last_address)| Kiosk {
                        id,
                        name,
                        enrolled_at,
                        revoked: revoked_at.is_some(),
                        last_seen_at,
                        last_address,
                    },
                )
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `DELETE /api/admin/kiosks/{id}`: revoke a kiosk. Its credential stops
/// working at once, and its last check-in vouches for nothing.
pub async fn revoke(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
) -> Response {
    let done = async {
        let mut tx = state.db.begin().await?;
        let name: Option<String> = sqlx::query_scalar(
            "UPDATE kiosks SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL RETURNING name",
        )
        .bind(now())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(name) = &name {
            audit::record(
                &mut tx,
                &Actor::user(admin.id, &admin.name),
                Event {
                    kind: AuditKind::KioskRevoked,
                    printer: None,
                    subject: None,
                    summary: format!("{} revoked the kiosk {name}", admin.name),
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok::<Option<String>, sqlx::Error>(name)
    }
    .await;
    match done {
        Ok(Some(_)) => {
            state.kiosks.forget(id);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "no such kiosk, or it is already revoked",
        ),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// The enrolled, unrevoked kiosk `headers` carry the credential of.
pub async fn kiosk_of(db: &SqlitePool, headers: &HeaderMap) -> Result<Option<i64>, sqlx::Error> {
    let Some(token) = kiosk_token(headers) else {
        return Ok(None);
    };
    sqlx::query_scalar("SELECT id FROM kiosks WHERE token_hash = ? AND revoked_at IS NULL")
        .bind(hash(&token))
        .fetch_optional(db)
        .await
}

/// `POST /api/kiosk/checkin`: a kiosk says it is here. POP believes where the
/// request came from, and nothing in it.
pub async fn checkin(State(state): State<AppState>, req: Request) -> Response {
    let id = match kiosk_of(&state.db, req.headers()).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "this screen is not an enrolled kiosk",
            );
        }
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let Some(from) =
        crate::access::client_address(&state.access, crate::access::peer(&req), req.headers())
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "POP cannot tell where this check-in came from",
        );
    };
    state.kiosks.saw(id, from);
    let written = sqlx::query("UPDATE kiosks SET last_seen_at = ?, last_address = ? WHERE id = ?")
        .bind(now())
        .bind(from.to_string())
        .bind(id)
        .execute(&state.db)
        .await;
    if let Err(e) = written {
        tracing::warn!("kiosks: recording a check-in: {e}");
    }
    StatusCode::NO_CONTENT.into_response()
}
