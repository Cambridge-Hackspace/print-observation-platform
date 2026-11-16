//! POP's server (ADR 0001): the API, live updates over Server-Sent Events, and
//! the web app, from one binary.

pub mod access;
pub mod api;
pub mod assets;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod config;
pub mod db;
pub mod events;
pub mod health;
pub mod notices;
pub mod notify;
pub mod pins;
pub mod printers;
pub mod run;
pub mod secrets;
pub mod tower;
pub mod users;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::Method,
    routing::{delete, get, post, put},
};
use sqlx::SqlitePool;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::{
    access::{Effect, Reach, Route, Who},
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
    /// Where the printers come from (#20, #21), when BAM! is configured.
    pub bam: Option<Arc<printers::BamSource>>,
    /// Seals the secrets POP keeps for printers (#21).
    pub secrets: secrets::Secrets,
    /// The printer category as BAM! last said, kept in sync (#22).
    pub bam_view: Option<tokio::sync::watch::Receiver<pop_integrations::sync::View>>,
    /// The tower's printers, as every screen is sent them (#49).
    pub tower: tokio::sync::watch::Receiver<Vec<api::TowerPrinter>>,
    /// Told when an admin decides about a printer, so the runner looks again.
    pub decided: Arc<tokio::sync::Notify>,
    /// The address phones reach POP at, if set (#49).
    pub public_url: Option<String>,
    /// Posts group notices (#68), once started.
    pub notifier: Option<notify::Notifier>,
    /// For POP's own requests out: Discord.
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(db: SqlitePool, assets: Arc<dyn Assets>) -> Self {
        Self {
            checks: Arc::new(vec![Arc::new(Database(db.clone()))]),
            db,
            events: Events::new(),
            assets,
            access: config::Access::default(),
            bam: None,
            secrets: secrets::Secrets::none(),
            bam_view: None,
            tower: tokio::sync::watch::channel(Vec::new()).1,
            decided: Arc::new(tokio::sync::Notify::new()),
            public_url: None,
            notifier: None,
            http: reqwest::Client::new(),
        }
    }

    /// Post group notices through this notifier (#68).
    pub fn with_notifier(mut self, notifier: notify::Notifier) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// The address phones reach POP at; empty for none.
    pub fn with_public_url(mut self, url: &str) -> Self {
        self.public_url = (!url.is_empty()).then(|| url.to_owned());
        self
    }

    /// Show the tower's printers as this runner runs them (#49).
    pub fn with_tower(
        mut self,
        tower: tokio::sync::watch::Receiver<Vec<api::TowerPrinter>>,
    ) -> Self {
        self.tower = tower;
        self
    }

    /// Follow BAM!'s printer category through this sync (#22), and check its
    /// live connection on /healthz.
    pub fn with_bam_sync(
        mut self,
        view: tokio::sync::watch::Receiver<pop_integrations::sync::View>,
    ) -> Self {
        self.bam_view = Some(view.clone());
        self.with_check(printers::BamLiveHealth(view))
    }

    /// Read printers from this BAM!, and check it on /healthz.
    pub fn with_bam(mut self, source: printers::BamSource) -> Self {
        let source = Arc::new(source);
        self.bam = Some(source.clone());
        self.with_check(printers::BamHealth(source))
    }

    /// Seal secrets with this.
    pub fn with_secrets(mut self, secrets: secrets::Secrets) -> Self {
        self.secrets = secrets;
        self
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
            public_url: self.public_url.clone(),
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
            who: Who::Anyone,
            handler: get(health::healthz),
        },
        Route {
            method: Method::GET,
            path: "/api/status",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            who: Who::Anyone,
            handler: get(status),
        },
        Route {
            method: Method::GET,
            path: "/api/events",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            who: Who::Anyone,
            handler: get(events::stream),
        },
        // ---- signing in and out (#11) ----
        Route {
            method: Method::POST,
            path: "/api/session",
            effect: Effect::Account,
            reach: Reach::Anywhere,
            who: Who::Anyone,
            handler: post(auth::sign_in),
        },
        Route {
            method: Method::DELETE,
            path: "/api/session",
            effect: Effect::Account,
            reach: Reach::Anywhere,
            who: Who::Member,
            handler: delete(auth::sign_out),
        },
        Route {
            method: Method::GET,
            path: "/api/me",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            who: Who::Member,
            handler: get(auth::me),
        },
        // ---- managing accounts (#11) ----
        Route {
            method: Method::GET,
            path: "/api/admin/users",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            who: Who::Admin,
            handler: get(auth::list),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/users",
            effect: Effect::Account,
            reach: Reach::Anywhere,
            who: Who::Admin,
            handler: post(auth::create),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/users/{id}/password",
            effect: Effect::Account,
            reach: Reach::Anywhere,
            who: Who::Admin,
            handler: put(auth::reset_password),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/users/{id}/role",
            effect: Effect::Account,
            reach: Reach::Anywhere,
            who: Who::Admin,
            handler: put(auth::set_role),
        },
        // ---- printers from BAM! (#21) ----
        Route {
            method: Method::GET,
            path: "/api/admin/printers",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            who: Who::Admin,
            handler: get(printers::list),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/printers/{bam_id}/add",
            // Which printers the tower runs: from inside the space only.
            effect: Effect::ControlsPrinter,
            reach: Reach::LocalOnly,
            who: Who::Admin,
            handler: post(printers::add),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/printers/{bam_id}/decline",
            effect: Effect::ControlsPrinter,
            reach: Reach::LocalOnly,
            who: Who::Admin,
            handler: post(printers::decline),
        },
        Route {
            method: Method::GET,
            path: "/api/admin/notify",
            effect: Effect::Read,
            reach: Reach::LocalOnly,
            who: Who::Admin,
            handler: get(notify::settings),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/notify/discord",
            // Where notices go, and a secret: from inside the space only.
            effect: Effect::Settings,
            reach: Reach::LocalOnly,
            who: Who::Admin,
            handler: put(notify::set_webhook),
        },
        Route {
            method: Method::DELETE,
            path: "/api/admin/notify/discord",
            effect: Effect::Settings,
            reach: Reach::LocalOnly,
            who: Who::Admin,
            handler: delete(notify::clear_webhook),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/notify/discord/test",
            effect: Effect::Settings,
            reach: Reach::LocalOnly,
            who: Who::Admin,
            handler: post(notify::test_webhook),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/printers/{bam_id}/access-code",
            // A printer's settings: from inside the space only (#7).
            effect: Effect::ControlsPrinter,
            reach: Reach::LocalOnly,
            who: Who::Admin,
            handler: put(printers::set_access_code),
        },
        // ---- the audit log (#8) ----
        Route {
            method: Method::GET,
            path: "/api/admin/audit",
            effect: Effect::Read,
            reach: Reach::Anywhere,
            who: Who::Admin,
            handler: get(auth::audit_log),
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
