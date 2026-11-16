//! POP's server (ADR 0001): the API, live updates over Server-Sent Events, and
//! the web app, from one binary.

pub mod api;
pub mod assets;
pub mod backup;
pub mod config;
pub mod db;
pub mod events;
pub mod health;

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use sqlx::SqlitePool;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::{
    api::Status,
    assets::Assets,
    events::Events,
    health::{Database, HealthCheck},
};

/// What every request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub events: Events,
    pub assets: Arc<dyn Assets>,
    /// What `/healthz` checks: the database, and each configured integration.
    pub checks: Arc<Vec<Arc<dyn HealthCheck>>>,
}

impl AppState {
    pub fn new(db: SqlitePool, assets: Arc<dyn Assets>) -> Self {
        Self {
            checks: Arc::new(vec![Arc::new(Database(db.clone()))]),
            db,
            events: Events::new(),
            assets,
        }
    }

    /// Add a check to `/healthz`, for an integration that is configured.
    pub fn with_check(mut self, check: impl HealthCheck + 'static) -> Self {
        let mut checks = (*self.checks).clone();
        checks.push(Arc::new(check));
        self.checks = Arc::new(checks);
        self
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
        .route("/healthz", get(health::healthz))
        .route("/api/status", get(status))
        .route("/api/events", get(events::stream))
        .fallback(assets::spa)
        .with_state(state)
        // One structured log line per request, at the configured level.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
}

async fn status(State(state): State<AppState>) -> Json<Status> {
    Json(state.status().await)
}
