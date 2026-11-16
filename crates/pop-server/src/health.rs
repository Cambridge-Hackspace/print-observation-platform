//! `GET /healthz` (#4): whether each part POP depends on is working, for a
//! supervisor, a load balancer, or a person.
//!
//! The database is always checked. Each integration adds a check of its own
//! when it is configured (BAM!, go2rtc, notifications), through
//! [`AppState::with_check`](crate::AppState::with_check). The answer is 200 when
//! every check passes and 503 when any fails, naming each.
//!
//! This route is reachable from outside the space, so a check says what is
//! wrong in a sentence fit for anyone to read, and logs the detail.

use std::{collections::BTreeMap, future::Future, pin::Pin};

use axum::{Json, extract::State, http::StatusCode};

use crate::{
    AppState,
    api::{CheckResult, Health},
};

pub type CheckFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// One part POP depends on.
pub trait HealthCheck: Send + Sync {
    /// Its name in the report: `database`, `bam`, `go2rtc`.
    fn name(&self) -> &'static str;
    /// `Ok` when it works; otherwise a sentence fit for anyone to read.
    fn check(&self) -> CheckFuture<'_>;
}

/// The database answers a query.
pub struct Database(pub sqlx::SqlitePool);

impl HealthCheck for Database {
    fn name(&self) -> &'static str {
        "database"
    }

    fn check(&self) -> CheckFuture<'_> {
        Box::pin(async move {
            sqlx::query("SELECT 1")
                .execute(&self.0)
                .await
                .map(|_| ())
                .map_err(|e| {
                    tracing::error!("health: the database does not answer: {e}");
                    "the database does not answer".to_owned()
                })
        })
    }
}

pub async fn healthz(State(state): State<AppState>) -> (StatusCode, Json<Health>) {
    let mut checks = BTreeMap::new();
    for c in state.checks.iter() {
        let r = c.check().await;
        checks.insert(
            c.name().to_owned(),
            CheckResult {
                ok: r.is_ok(),
                error: r.err(),
            },
        );
    }
    let ok = checks.values().all(|c| c.ok);
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(Health { ok, checks }))
}
