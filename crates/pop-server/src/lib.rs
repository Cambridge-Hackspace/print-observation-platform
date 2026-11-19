//! POP's server (ADR 0001): the API, live updates over Server-Sent Events, and
//! the web app, from one binary.

pub mod access;
pub mod api;
pub mod assets;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod cameras;
pub mod config;
pub mod db;
pub mod events;
pub mod feedback;
pub mod guides;
pub mod health;
pub mod keys;
pub mod kiosks;
pub mod notices;
pub mod notify;
pub mod pins;
pub mod printers;
pub mod report;
pub mod run;
pub mod secrets;
pub mod tower;
pub mod updater;
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
    access::{Class, Effect, Need, Reach, Route},
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
    /// Where each enrolled kiosk last checked in from (#12): what makes a
    /// network the space's.
    pub kiosks: kiosks::Sightings,
    /// Where the printers come from (#20, #21), when BAM! is configured.
    pub bam: Option<Arc<printers::BamSource>>,
    /// Seals the secrets POP keeps for printers (#21).
    pub secrets: secrets::Secrets,
    /// The printer category as BAM! last said, kept in sync (#22).
    pub bam_view: Option<tokio::sync::watch::Receiver<pop_integrations::sync::View>>,
    /// The tower's printers, as every screen is sent them (#49).
    pub tower: tokio::sync::watch::Receiver<Vec<api::TowerPrinter>>,
    /// What the group chat was told about printers needing a hand (#55).
    pub told: tokio::sync::watch::Receiver<Vec<api::GroupNotice>>,
    /// Told when an admin decides about a printer, so the runner looks again.
    pub decided: Arc<tokio::sync::Notify>,
    /// The address phones reach POP at, if set (#49).
    pub public_url: Option<String>,
    /// Posts group notices (#68), once started.
    pub notifier: Option<notify::Notifier>,
    /// For POP's own requests out: Discord.
    pub http: reqwest::Client,
    /// Where stored files live: the data directory's files/ (#3, #71).
    pub files_dir: Option<std::path::PathBuf>,
    /// How published guides are offered to everyone (#94).
    pub guides: config::Guides,
    /// Each printer's camera and its health, when go2rtc is configured (#66).
    pub cameras: Option<tokio::sync::watch::Receiver<Vec<api::AdminCamera>>>,
    /// The go2rtc viewers watch through (#67).
    pub go2rtc: Option<pop_integrations::go2rtc::Go2rtc>,
    /// Places for viewers, across every camera.
    pub viewers: Arc<tokio::sync::Semaphore>,
    /// The cameras POP reads itself (#33).
    pub jpegs: cameras::Jpegs,
    /// What the public live stream carries (#134).
    pub public: config::Public,
    /// How kiosk sessions behave (#12).
    pub kiosk: config::Kiosk,
    /// Feedback, filed as GitHub issues, when configured (#96).
    pub feedback: Option<Arc<feedback::Feedback>>,
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
            told: tokio::sync::watch::channel(Vec::new()).1,
            decided: Arc::new(tokio::sync::Notify::new()),
            kiosks: kiosks::Sightings::default(),
            public_url: None,
            notifier: None,
            http: reqwest::Client::new(),
            files_dir: None,
            guides: config::Guides::default(),
            cameras: None,
            go2rtc: None,
            viewers: Arc::new(tokio::sync::Semaphore::new(cameras::MAX_VIEWERS)),
            jpegs: cameras::Jpegs::default(),
            public: config::Public::default(),
            kiosk: config::Kiosk::default(),
            feedback: None,
        }
    }

    /// Show the cameras as this keeper keeps them (#66), and check go2rtc on
    /// /healthz.
    pub fn with_cameras(
        mut self,
        kept: cameras::Kept,
        go2rtc: pop_integrations::go2rtc::Go2rtc,
    ) -> Self {
        self.cameras = Some(kept.health);
        self.jpegs = kept.jpegs;
        self.go2rtc = Some(go2rtc.clone());
        self.with_check(cameras::Go2rtcHealth(go2rtc))
    }

    /// File feedback through this (#96).
    pub fn with_feedback(mut self, feedback: Arc<feedback::Feedback>) -> Self {
        self.feedback = Some(feedback);
        self
    }

    /// Carry the public live stream under these settings (#134).
    /// Kiosk sessions as configured (#12).
    pub fn with_kiosk(mut self, kiosk: config::Kiosk) -> Self {
        self.kiosk = kiosk;
        self
    }

    pub fn with_public(mut self, public: config::Public) -> Self {
        self.public = public;
        self
    }

    /// Offer published guides under these settings (#94).
    pub fn with_guides(mut self, guides: config::Guides) -> Self {
        self.guides = guides;
        self
    }

    /// Keep stored files (guide images) under `dir`.
    pub fn with_files_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.files_dir = Some(dir);
        self
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

    /// Show what the group chat was told, as these notices tell it (#55).
    pub fn with_told(mut self, told: tokio::sync::watch::Receiver<Vec<api::GroupNotice>>) -> Self {
        self.told = told;
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
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: get(health::healthz),
        },
        Route {
            method: Method::GET,
            path: "/api/status",
            effect: Effect::Read,
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: get(status),
        },
        Route {
            method: Method::GET,
            path: "/api/events",
            effect: Effect::Read,
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: get(events::stream),
        },
        // ---- signing in and out (#11) ----
        Route {
            method: Method::POST,
            path: "/api/session",
            effect: Effect::Account,
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: post(auth::sign_in),
        },
        Route {
            method: Method::DELETE,
            path: "/api/session",
            effect: Effect::Account,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: delete(auth::sign_out),
        },
        Route {
            method: Method::GET,
            path: "/api/session",
            effect: Effect::Read,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: get(auth::session),
        },
        // ---- personal API keys (#13) ----
        // Each person's own: made, listed and revoked signed in, from
        // anywhere, as their other account settings are.
        Route {
            method: Method::GET,
            path: "/api/keys",
            effect: Effect::Read,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: get(keys::list),
        },
        Route {
            method: Method::POST,
            path: "/api/keys",
            effect: Effect::Account,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: post(keys::create),
        },
        Route {
            method: Method::DELETE,
            path: "/api/keys/{id}",
            effect: Effect::Account,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: delete(keys::revoke),
        },
        // What a slicer asks first, with a key, to test the connection
        // (OctoPrint's; #37). The one route a key reaches so far.
        Route {
            method: Method::GET,
            path: "/api/version",
            effect: Effect::Read,
            need: Need::Key,
            reach: Reach::Anywhere,
            handler: get(keys::version),
        },
        Route {
            method: Method::GET,
            path: "/api/me",
            effect: Effect::Read,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: get(auth::me),
        },
        // ---- managing accounts (#11) ----
        Route {
            method: Method::GET,
            path: "/api/admin/users",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(auth::list),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/users",
            effect: Effect::Account,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: post(auth::create),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/users/{id}/password",
            effect: Effect::Account,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(auth::reset_password),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/users/{id}/role",
            effect: Effect::Account,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(auth::set_role),
        },
        // ---- managing local users (#90) ----
        Route {
            method: Method::PUT,
            path: "/api/admin/users/{id}/name",
            effect: Effect::Account,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(auth::set_name),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/users/{id}/active",
            effect: Effect::Account,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(auth::set_active),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/users/{id}/training",
            effect: Effect::Account,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(auth::set_training),
        },
        // ---- printers from BAM! (#21) ----
        Route {
            method: Method::GET,
            path: "/api/admin/printers",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(printers::list),
        },
        // ---- feedback from any screen (#96) ----
        Route {
            method: Method::GET,
            path: "/api/feedback",
            effect: Effect::Read,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: get(feedback::settings),
        },
        Route {
            method: Method::POST,
            path: "/api/feedback",
            // A report: it reaches GitHub, never a printer. The role check,
            // feedback.file, is the handler's, from [feedback] roles.
            effect: Effect::Report,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: post(feedback::file).layer(axum::extract::DefaultBodyLimit::max(8 << 20)),
        },
        Route {
            method: Method::GET,
            path: "/api/feedback/{id}",
            effect: Effect::Read,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: get(feedback::status),
        },
        // ---- "something looks wrong" (#63): told to the group, never the printer ----
        Route {
            method: Method::POST,
            path: "/api/printers/{bam_id}/report",
            effect: Effect::Report,
            need: Need::Account,
            reach: Reach::Anywhere,
            handler: post(report::report),
        },
        // ---- watching a camera (#67, #58): visibility-class ----
        Route {
            method: Method::GET,
            path: "/api/printers/{bam_id}/camera/mse",
            effect: Effect::Read,
            need: Need::Class(Class::Visibility),
            // Visibility-class: anyone at the space, any member online (#12).
            reach: Reach::Anywhere,
            handler: get(cameras::watch_mse),
        },
        Route {
            method: Method::GET,
            path: "/api/printers/{bam_id}/camera/mjpeg",
            effect: Effect::Read,
            need: Need::Class(Class::Visibility),
            // Visibility-class: anyone at the space, any member online (#12).
            reach: Reach::Anywhere,
            handler: get(cameras::watch_mjpeg),
        },
        Route {
            method: Method::GET,
            path: "/api/admin/cameras",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(cameras::list),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/printers/{bam_id}/add",
            // Which printers the tower runs: admin-class, from anywhere (#12).
            effect: Effect::ControlsPrinter,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: post(printers::add),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/printers/{bam_id}/decline",
            effect: Effect::ControlsPrinter,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: post(printers::decline),
        },
        // Published guides, for anyone, from anywhere (#94): read-only, like
        // every route a stranger can reach (ADR 0001).
        Route {
            method: Method::GET,
            path: "/api/public/guides",
            effect: Effect::Read,
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: get(guides::public_list),
        },
        Route {
            method: Method::GET,
            path: "/api/public/guides/{slug}",
            effect: Effect::Read,
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: get(guides::public_get),
        },
        Route {
            method: Method::GET,
            path: "/api/public/guides/{slug}/images/{id}",
            effect: Effect::Read,
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: get(guides::public_image),
        },
        Route {
            method: Method::GET,
            path: "/api/public/error-guides",
            effect: Effect::Read,
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: get(guides::public_error_guides),
        },
        Route {
            method: Method::GET,
            path: "/api/admin/error-guides",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(guides::admin_error_guides),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/error-guides/{key}",
            effect: Effect::Content,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(guides::admin_link_error_guide),
        },
        // Guides (#71): written by admins, from anywhere; nothing here
        // reaches a printer.
        Route {
            method: Method::GET,
            path: "/api/admin/guides",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(guides::admin_list),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/guides",
            effect: Effect::Content,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: post(guides::admin_create),
        },
        Route {
            method: Method::GET,
            path: "/api/admin/guides/{slug}",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(guides::admin_get),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/guides/{slug}",
            effect: Effect::Content,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(guides::admin_update),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/guides/{slug}/images",
            effect: Effect::Content,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: post(guides::admin_add_image),
        },
        Route {
            method: Method::GET,
            path: "/api/admin/guides/{slug}/images/{id}",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(guides::admin_image),
        },
        Route {
            method: Method::GET,
            path: "/api/admin/notify",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(notify::settings),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/notify/discord",
            // Where notices go, and a secret: admin-class, from anywhere (#12).
            effect: Effect::Settings,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(notify::set_webhook),
        },
        Route {
            method: Method::DELETE,
            path: "/api/admin/notify/discord",
            effect: Effect::Settings,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: delete(notify::clear_webhook),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/notify/discord/test",
            effect: Effect::Settings,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: post(notify::test_webhook),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/printers/{bam_id}/access-code",
            // A printer's settings: admin-class, from anywhere (#12).
            effect: Effect::ControlsPrinter,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(printers::set_access_code),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/printers/{bam_id}/camera-login",
            // A printer's settings: admin-class, as its access code (#150).
            effect: Effect::ControlsPrinter,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(printers::set_camera_login),
        },
        Route {
            method: Method::DELETE,
            path: "/api/admin/printers/{bam_id}/camera-login",
            effect: Effect::ControlsPrinter,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: delete(printers::clear_camera_login),
        },
        Route {
            method: Method::PUT,
            path: "/api/admin/printers/{bam_id}/printer-login",
            // A printer's settings: admin-class, as its access code (#29, #30).
            effect: Effect::ControlsPrinter,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: put(printers::set_printer_login),
        },
        Route {
            method: Method::DELETE,
            path: "/api/admin/printers/{bam_id}/printer-login",
            effect: Effect::ControlsPrinter,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: delete(printers::clear_printer_login),
        },
        // ---- automatic upgrades (#148) ----
        Route {
            method: Method::GET,
            path: "/api/updater/quiet",
            effect: Effect::Read,
            // For the updater, on this machine: it says which printers are
            // printing, and nothing more.
            need: Need::Anyone,
            reach: Reach::ThisMachine,
            handler: get(updater::quiet),
        },
        Route {
            method: Method::GET,
            path: "/api/admin/updater",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(updater::status),
        },
        // ---- kiosks (#12): the print stations, and their word ----
        Route {
            method: Method::POST,
            path: "/api/kiosk/checkin",
            // Its credential is its need: a kiosk that is not enrolled is
            // refused by the handler, and what it vouches for is where the
            // request came from, never anything in it.
            effect: Effect::Read,
            need: Need::Anyone,
            reach: Reach::Anywhere,
            handler: post(kiosks::checkin),
        },
        Route {
            method: Method::GET,
            path: "/api/admin/kiosks",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: get(kiosks::list),
        },
        Route {
            method: Method::POST,
            path: "/api/admin/kiosks",
            effect: Effect::Settings,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: post(kiosks::enroll),
        },
        Route {
            method: Method::DELETE,
            path: "/api/admin/kiosks/{id}",
            effect: Effect::Settings,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
            handler: delete(kiosks::revoke),
        },
        // ---- the audit log (#8) ----
        Route {
            method: Method::GET,
            path: "/api/admin/audit",
            effect: Effect::Read,
            need: Need::Class(Class::Admin),
            reach: Reach::Anywhere,
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
