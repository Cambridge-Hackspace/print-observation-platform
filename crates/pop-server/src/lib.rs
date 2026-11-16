//! POP's server (ADR 0001): the API, live updates over Server-Sent Events, and
//! the web app, from one binary.

pub mod access;
pub mod api;
pub mod assets;
pub mod backup;
pub mod config;
pub mod db;
pub mod events;
pub mod health;

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::Method, routing::get};
use sqlx::SqlitePool;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::{
    access::{Effect, Reach, Route},
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
    /// Which requests are local (#7).
    pub access: config::Access,
}

impl AppState {
    pub fn new(db: SqlitePool, assets: Arc<dyn Assets>) -> Self {
        Self {
            checks: Arc::new(vec![Arc::new(Database(db.clone()))]),
            db,
            events: Events::new(),
            assets,
            access: config::Access::default(),
        }
    }

    /// Use these networks and proxies to tell local requests from remote ones.
    /// Without this, only this machine is local.
    pub fn with_access(mut self, access: config::Access) -> Self {
        self.access = access;
        self
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

/// Every route POP serves, with what it does and from where it may be
/// reached (#7). The web app's fallback, which serves files and changes
/// nothing, is outside the table.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            method: Method::GET,
            path: "/healthz",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            handler: get(health::healthz),
        },
        Route {
            method: Method::GET,
            path: "/api/status",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            handler: get(status),
        },
        Route {
            method: Method::GET,
            path: "/api/events",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            handler: get(events::stream),
        },
    ]
}

/// Every route POP serves.
pub fn router(state: AppState) -> Router {
    router_with(routes(), state)
}

/// A router serving `table`, for a test that needs routes of its own.
pub fn router_with(table: Vec<Route>, state: AppState) -> Router {
    access::build(table, &state)
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
