//! POP's server (ADR 0001): the API, live updates over Server-Sent Events, and
//! the web app, from one binary.

pub mod api;
pub mod assets;
pub mod backup;
pub mod config;
pub mod db;
pub mod events;

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use sqlx::SqlitePool;

use crate::{api::Status, assets::Assets, events::Events};

/// What every request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub events: Events,
    pub assets: Arc<dyn Assets>,
}

impl AppState {
    pub fn new(db: SqlitePool, assets: Arc<dyn Assets>) -> Self {
        Self {
            db,
            events: Events::new(),
            assets,
        }
    }

    /// The server's status, as `/api/status` and the live stream report it.
    pub async fn status(&self) -> Status {
        Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            // A database that cannot answer this is reported as schema 0
            // rather than taking the status down with it; /healthz is what
            // fails on a broken database.
            schema_version: db::schema_version(&self.db).await.unwrap_or(0),
        }
    }
}

/// Every route POP serves.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/status", get(status))
        .route("/api/events", get(events::stream))
        .fallback(assets::spa)
        .with_state(state)
}

/// Liveness for a supervisor or a load balancer: 200 when the database
/// answers, 503 when it does not.
async fn healthz(State(state): State<AppState>) -> (axum::http::StatusCode, &'static str) {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => (axum::http::StatusCode::OK, "ok"),
        Err(_) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "database unavailable",
        ),
    }
}

async fn status(State(state): State<AppState>) -> Json<Status> {
    Json(state.status().await)
}
