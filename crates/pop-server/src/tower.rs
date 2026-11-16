//! Which of BAM!'s printers the tower runs (#23).
//!
//! A printer that appears in the BAM! category is *offered* to admins: they
//! add it to the tower, or say it is not one. Nothing is added silently, and
//! only a printer with every field right can be added (#21). A printer
//! archived in BAM! is *retired*: it leaves the tower, but its row and its
//! history stay. Restored in BAM!, it is offered again.

use std::collections::HashMap;

use pop_integrations::bam::Asset;
use sqlx::SqlitePool;

use crate::{
    api::{AuditKind, TowerState},
    audit::{self, Actor, Event},
    users::now,
};

fn state_str(s: TowerState) -> &'static str {
    match s {
        TowerState::Added => "added",
        TowerState::Declined => "declined",
        TowerState::Retired => "retired",
        TowerState::Offered => "offered",
    }
}

fn state_from(s: &str) -> TowerState {
    match s {
        "added" => TowerState::Added,
        "declined" => TowerState::Declined,
        "retired" => TowerState::Retired,
        _ => TowerState::Offered,
    }
}

/// Each decided printer's state, by BAM! id. One not here is offered.
pub async fn states(db: &SqlitePool) -> Result<HashMap<String, TowerState>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT bam_id, state FROM tower_printers")
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, s)| (id, state_from(&s)))
        .collect())
}

/// Record a decision about a printer, with its audit entry, as one.
pub async fn decide(
    db: &SqlitePool,
    bam_id: &str,
    name: &str,
    to: TowerState,
    by: &Actor,
    summary: String,
) -> Result<(), sqlx::Error> {
    let kind = match to {
        TowerState::Added => AuditKind::PrinterAdded,
        TowerState::Declined => AuditKind::PrinterDeclined,
        TowerState::Retired | TowerState::Offered => AuditKind::PrinterRetired,
    };
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO tower_printers (bam_id, state, name, decided_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (bam_id) DO UPDATE SET state = excluded.state, name = excluded.name,
                                            decided_at = excluded.decided_at",
    )
    .bind(bam_id)
    .bind(state_str(to))
    .bind(name)
    .bind(now())
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        by,
        Event {
            kind,
            printer: Some(bam_id),
            subject: None,
            summary,
        },
    )
    .await?;
    tx.commit().await
}

/// BAM!, as the actor of what follows from what happens there.
pub fn bam_actor() -> Actor {
    Actor {
        id: None,
        name: Some("BAM!".to_owned()),
    }
}

/// Retire every added printer BAM! has archived. Returns their ids.
pub async fn retire_archived(
    db: &SqlitePool,
    archived: &[Asset],
) -> Result<Vec<String>, sqlx::Error> {
    let states = states(db).await?;
    let mut retired = Vec::new();
    for a in archived {
        if states.get(&a.asset.id) == Some(&TowerState::Added) {
            decide(
                db,
                &a.asset.id,
                &a.asset.name,
                TowerState::Retired,
                &bam_actor(),
                format!(
                    "{} was archived in BAM!, so it is retired from the tower",
                    a.asset.name
                ),
            )
            .await?;
            retired.push(a.asset.id.clone());
        }
    }
    Ok(retired)
}

/// Follow BAM!'s view, retiring each added printer it archives (#22, #23).
pub fn spawn_retirer(
    db: SqlitePool,
    mut view: tokio::sync::watch::Receiver<pop_integrations::sync::View>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let archived = view.borrow_and_update().archived.clone();
            if let Err(e) = retire_archived(&db, &archived).await {
                tracing::error!("retiring printers archived in BAM!: {e}");
            }
            if view.changed().await.is_err() {
                return;
            }
        }
    })
}
