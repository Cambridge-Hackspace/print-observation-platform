//! Printers from BAM!, as admins see them (#21): each ready to run, or its
//! problems, each naming the BAM! field to put right. A printer is never
//! half-added.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pop_integrations::{
    bam::{Bam, BamError},
    printers::{Connection, FieldMap, printer_from},
};

use crate::{
    AppState,
    api::{AccessCode, AdminPrinter, ApiError, AuditKind, PrinterProblem, User},
    audit::{self, Actor, Event},
    health::{CheckFuture, HealthCheck},
    secrets::{SecretError, Secrets},
};

/// BAM!, as POP is configured to read it.
pub struct BamSource {
    pub client: Bam,
    pub category: String,
    pub fields: FieldMap,
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

fn kind(c: &Connection) -> &'static str {
    match c {
        Connection::Moonraker { .. } => "moonraker",
        Connection::OctoPrint { .. } => "octoprint",
        Connection::PrusaLink { .. } => "prusalink",
        Connection::Bambu { .. } => "bambu",
        Connection::UsbSerial { .. } => "usb_serial",
    }
}

/// `GET /api/admin/printers`: every printer in the BAM! category, ready or
/// not, with what is wrong with each that is not.
pub async fn list(State(state): State<AppState>) -> Response {
    let Some(bam) = state.bam.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP has no BAM! to read printers from: set [bam] url, category and POP_BAM_TOKEN",
        );
    };
    let assets = match bam.client.assets(&bam.category, false).await {
        Ok(a) => a,
        Err(e) => return error(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let held = match Secrets::with_access_codes(&state.db).await {
        Ok(h) => h,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let has_code = |id: &str| held.iter().any(|h| h == id);
    let mut out: Vec<AdminPrinter> = assets
        .iter()
        .map(|a| match printer_from(a, &bam.fields, &has_code) {
            Ok(p) => AdminPrinter {
                bam_id: p.bam_id,
                name: p.name,
                ready: true,
                connection: Some(kind(&p.connection).to_owned()),
                problems: vec![],
            },
            Err(problems) => AdminPrinter {
                bam_id: a.asset.id.clone(),
                name: a.asset.name.clone(),
                ready: false,
                connection: a.field_values.get(&bam.fields.connection).cloned(),
                problems: problems
                    .into_iter()
                    .map(|p| PrinterProblem {
                        field: p.field,
                        problem: p.problem,
                    })
                    .collect(),
            },
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    Json(out).into_response()
}

/// `PUT /api/admin/printers/{bam_id}/access-code`: a Bambu printer's LAN
/// access code, kept sealed in POP and never in BAM!.
pub async fn set_access_code(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(bam_id): Path<String>,
    body: Result<Json<AccessCode>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let code = body.access_code.trim();
    if code.len() < 4 || code.len() > 32 || code.contains(char::is_whitespace) {
        return error(
            StatusCode::BAD_REQUEST,
            "access_code: the code from the printer's network settings, 4 to 32 characters without spaces",
        );
    }
    // The printer's name, for the record; its id is enough if BAM! cannot say.
    let name = match state.bam.as_ref() {
        Some(b) => match b.client.asset(&bam_id).await {
            Ok(a) => a.asset.name,
            Err(BamError::NotFound) => {
                return error(
                    StatusCode::NOT_FOUND,
                    "BAM! has no such printer, or POP's token may not see it",
                );
            }
            Err(_) => bam_id.clone(),
        },
        None => bam_id.clone(),
    };
    if let Err(e) = state
        .secrets
        .set_access_code(&state.db, &bam_id, code)
        .await
    {
        let status = if e == SecretError::NoKey {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        return error(status, e.to_string());
    }
    let mut conn = match state.db.acquire().await {
        Ok(c) => c,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let recorded = audit::record(
        &mut conn,
        &Actor::user(admin.id, &admin.name),
        Event {
            kind: AuditKind::AccessCodeSet,
            printer: Some(&bam_id),
            subject: None,
            // Never the code itself.
            summary: format!("{} entered {name}'s LAN access code", admin.name),
        },
    )
    .await;
    match recorded {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// BAM! answers, takes POP's token, and has the printer category.
pub struct BamHealth(pub Arc<BamSource>);

impl HealthCheck for BamHealth {
    fn name(&self) -> &'static str {
        "bam"
    }

    fn check(&self) -> CheckFuture<'_> {
        Box::pin(async move {
            self.0.client.check(&self.0.category).await.map_err(|e| {
                tracing::error!("health: {e}");
                // A sentence for anyone to read: no address, no token.
                match e {
                    BamError::Token(_) => "BAM! refused POP's token".to_owned(),
                    BamError::Permission(_) => "POP's BAM! token lacks a permission".to_owned(),
                    BamError::Unreachable(_) => "BAM! cannot be reached".to_owned(),
                    _ => "BAM! does not have the printer category".to_owned(),
                }
            })
        })
    }
}
