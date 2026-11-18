//! POP's side of automatic upgrades (#148). The updater is its own service
//! (crates/pop-updater), so a POP that will not start can still be put back;
//! POP tells it when the tower is quiet, and records what it did.
//!
//! - `GET /api/updater/quiet`, from this machine only: the running version,
//!   and the printers busy with a print. The updater applies nothing while
//!   any is.
//! - The updater writes each thing it does as a line of
//!   `<data_dir>/updater/events.jsonl`, whether POP is running or not. POP
//!   reads it every half minute and at start, and records each line once,
//!   in the audit log and as the last result admins see
//!   (`GET /api/admin/updater`).

use std::path::Path;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pop_printers::PrinterStatus;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::{
    AppState,
    api::{ApiError, AuditKind, TowerPrinter, UpdaterEvent, UpdaterQuiet, UpdaterStatus},
    audit::{self, Actor, Event},
};

/// Where the updater writes what it does, under POP's data directory.
pub const EVENTS: &str = "updater/events.jsonl";

/// The printers busy with a print: printing, paused, or stopped mid-print
/// for a person. An upgrade waits for every one.
pub fn busy(printers: &[TowerPrinter]) -> Vec<String> {
    printers
        .iter()
        .filter(|p| match p.state.status {
            PrinterStatus::Printing | PrinterStatus::Paused => true,
            PrinterStatus::NeedsHand => p.state.job.is_some(),
            PrinterStatus::Offline | PrinterStatus::Idle | PrinterStatus::Done => false,
        })
        .map(|p| p.name.clone())
        .collect()
}

/// `GET /api/updater/quiet`.
pub async fn quiet(State(state): State<AppState>) -> Json<UpdaterQuiet> {
    let busy = busy(&state.tower.borrow());
    Json(UpdaterQuiet {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        quiet: busy.is_empty(),
        busy,
    })
}

/// `GET /api/admin/updater`.
pub async fn status(State(state): State<AppState>) -> Response {
    match last(&state.db).await {
        Ok(last) => Json(UpdaterStatus {
            running: env!("CARGO_PKG_VERSION").to_owned(),
            last,
        })
        .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn last(db: &SqlitePool) -> Result<Option<UpdaterEvent>, sqlx::Error> {
    let row: Option<(i64, String, Option<String>, String)> = sqlx::query_as(
        "SELECT at, kind, version, summary FROM updater_events ORDER BY at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(at, kind, version, summary)| UpdaterEvent {
        at,
        kind,
        version,
        summary,
    }))
}

/// One line of the updater's record.
#[derive(Deserialize)]
struct Line {
    id: String,
    at: i64,
    kind: String,
    version: Option<String>,
    summary: String,
}

/// The audit kind for what the updater did; None for a kind this POP does
/// not know, which is still recorded, as Other.
fn audit_kind(kind: &str) -> AuditKind {
    match kind {
        "checked" => AuditKind::UpdateChecked,
        "waiting" => AuditKind::UpdateWaited,
        "downloaded" => AuditKind::UpdateDownloaded,
        "refused" => AuditKind::UpdateRefused,
        "applied" => AuditKind::UpdateApplied,
        "rolled_back" => AuditKind::UpdateRolledBack,
        "failed" => AuditKind::UpdateFailed,
        _ => AuditKind::Other,
    }
}

/// Record each line of the updater's record not recorded yet; how many.
pub async fn ingest(db: &SqlitePool, data_dir: &Path) -> Result<usize, String> {
    let text = match tokio::fs::read_to_string(data_dir.join(EVENTS)).await {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("reading the updater's record: {e}")),
    };
    let mut recorded = 0;
    for (n, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let line: Line = match serde_json::from_str(raw) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("the updater's record, line {}: {e}", n + 1);
                continue;
            }
        };
        let mut tx = db.begin().await.map_err(|e| e.to_string())?;
        let new = sqlx::query(
            "INSERT INTO updater_events (id, at, kind, version, summary) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(&line.id)
        .bind(line.at)
        .bind(&line.kind)
        .bind(&line.version)
        .bind(&line.summary)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        .rows_affected()
            == 1;
        if new {
            audit::record(
                &mut tx,
                &Actor::updater(),
                Event {
                    kind: audit_kind(&line.kind),
                    printer: None,
                    subject: None,
                    summary: line.summary.clone(),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
            recorded += 1;
        }
        tx.commit().await.map_err(|e| e.to_string())?;
    }
    Ok(recorded)
}

/// Read the updater's record now, and every `every` after.
pub fn spawn(db: SqlitePool, data_dir: std::path::PathBuf, every: std::time::Duration) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = ingest(&db, &data_dir).await {
                tracing::error!("{e}");
            }
            tokio::time::sleep(every).await;
        }
    });
}
