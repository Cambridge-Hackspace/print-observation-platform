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
    printers::{Camera, Connection, FieldMap, printer_from},
};

use crate::{
    AppState,
    api::{
        AccessCode, AdminPrinter, ApiError, AuditKind, CameraLoginBody, PrinterProblem, TowerState,
        User,
    },
    audit::{self, Actor, Event},
    health::{CheckFuture, HealthCheck},
    secrets::{CameraLogin, SecretError, Secrets},
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
    let logins = match Secrets::with_camera_logins(&state.db).await {
        Ok(h) => h,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    // A login counts only for the address it was entered for.
    let mut fitting = Vec::new();
    for a in &assets {
        if !logins.contains(&a.asset.id) {
            continue;
        }
        if let Ok(Some(Camera::Address { url })) =
            printer_from(a, &bam.fields, &has_code).map(|p| p.camera)
            && let Ok(Some(l)) = state.secrets.camera_login(&state.db, &a.asset.id).await
            && l.origin == crate::cameras::origin(&url)
        {
            fitting.push(a.asset.id.clone());
        }
    }
    let has_login = |id: &str| fitting.iter().any(|h| h == id);
    let decided = match crate::tower::states(&state.db).await {
        Ok(d) => d,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    // Retired, but back in BAM!: offered again.
    let tower = |id: &str| match decided.get(id) {
        None | Some(TowerState::Retired) => TowerState::Offered,
        Some(s) => *s,
    };
    // A field as BAM! has it, when it has one that is not blank.
    let field = |a: &pop_integrations::bam::Asset, key: &str| {
        a.field_values
            .get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let mut out: Vec<AdminPrinter> = assets
        .iter()
        .map(|a| match printer_from(a, &bam.fields, &has_code) {
            Ok(p) => AdminPrinter {
                bam_id: p.bam_id,
                name: p.name,
                ready: true,
                connection: Some(kind(&p.connection).to_owned()),
                problems: vec![],
                tower: tower(&a.asset.id),
                categories: a.categories.iter().map(|c| c.name.clone()).collect(),
                training: p.training,
                camera_login_fits: matches!(p.camera, Some(Camera::Address { .. })),
                camera: p.camera.is_some(),
                access_code: has_code(&a.asset.id),
                camera_login: has_login(&a.asset.id),
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
                        in_pop: p.in_pop,
                    })
                    .collect(),
                tower: tower(&a.asset.id),
                categories: a.categories.iter().map(|c| c.name.clone()).collect(),
                training: field(a, &bam.fields.training),
                camera: field(a, &bam.fields.camera).is_some(),
                access_code: has_code(&a.asset.id),
                // Not until BAM! has the printer right.
                camera_login_fits: false,
                camera_login: has_login(&a.asset.id),
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
    // The tower runs it with the code now, not at BAM!'s next change.
    state.decided.notify_one();
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

/// The printer's name, for the record; its id when BAM! cannot say. None
/// when BAM! says it has no such printer.
async fn name_of(state: &AppState, bam_id: &str) -> Option<String> {
    match state.bam.as_ref() {
        Some(b) => match b.client.asset(bam_id).await {
            Ok(a) => Some(a.asset.name),
            Err(BamError::NotFound) => None,
            Err(_) => Some(bam_id.to_owned()),
        },
        None => Some(bam_id.to_owned()),
    }
}

/// Record `kind` about `bam_id`, said by `summary`; 204, or why not.
async fn recorded(
    state: &AppState,
    admin: &User,
    kind: AuditKind,
    bam_id: &str,
    summary: String,
) -> Response {
    let mut conn = match state.db.acquire().await {
        Ok(c) => c,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let done = audit::record(
        &mut conn,
        &Actor::user(admin.id, &admin.name),
        Event {
            kind,
            printer: Some(bam_id),
            subject: None,
            summary,
        },
    )
    .await;
    match done {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `PUT /api/admin/printers/{bam_id}/camera-login` (#150): the login for a
/// printer's camera, kept sealed in POP so BAM! holds only its address.
pub async fn set_camera_login(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(bam_id): Path<String>,
    body: Result<Json<CameraLoginBody>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let username = body.username.trim();
    if username.chars().count() > 64 || username.chars().any(char::is_control) {
        return error(
            StatusCode::BAD_REQUEST,
            "username: the camera's user, up to 64 characters; empty if it asks for a password only",
        );
    }
    if body.password.is_empty()
        || body.password.chars().count() > 128
        || body.password.chars().any(char::is_control)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "password: the camera's password, 1 to 128 characters",
        );
    }
    // The camera's address, from BAM!: the login is bound to it.
    let Some(bam) = state.bam.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP has no BAM! to read the camera's address from",
        );
    };
    let asset = match bam.client.asset(&bam_id).await {
        Ok(a) => a,
        Err(BamError::NotFound) => {
            return error(
                StatusCode::NOT_FOUND,
                "BAM! has no such printer, or POP's token may not see it",
            );
        }
        Err(e) => return error(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let held = Secrets::with_access_codes(&state.db)
        .await
        .unwrap_or_default();
    let has_code = |id: &str| held.iter().any(|h| h == id);
    let url = match printer_from(&asset, &bam.fields, &has_code).map(|p| p.camera) {
        Ok(Some(Camera::Address { url })) => url,
        _ => {
            return error(
                StatusCode::CONFLICT,
                "BAM! gives this printer no camera address a login goes with: set its camera field to the camera's address, without the password",
            );
        }
    };
    let name = asset.asset.name;
    let login = CameraLogin {
        username: username.to_owned(),
        password: body.password,
        origin: crate::cameras::origin(&url),
    };
    if let Err(e) = state
        .secrets
        .set_camera_login(&state.db, &bam_id, &login)
        .await
    {
        let status = if e == SecretError::NoKey {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        return error(status, e.to_string());
    }
    // go2rtc is given the login now, not at BAM!'s next change.
    state.decided.notify_one();
    // Never the password.
    let summary = format!("{} entered the login for {name}'s camera", admin.name);
    recorded(&state, &admin, AuditKind::CameraLoginSet, &bam_id, summary).await
}

/// `DELETE /api/admin/printers/{bam_id}/camera-login` (#150).
pub async fn clear_camera_login(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(bam_id): Path<String>,
) -> Response {
    match Secrets::clear_camera_login(&state.db, &bam_id).await {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::NOT_FOUND, "POP holds no login for that camera"),
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
    state.decided.notify_one();
    let name = name_of(&state, &bam_id)
        .await
        .unwrap_or_else(|| bam_id.clone());
    let summary = format!("{} removed the login for {name}'s camera", admin.name);
    recorded(
        &state,
        &admin,
        AuditKind::CameraLoginCleared,
        &bam_id,
        summary,
    )
    .await
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

/// BAM!'s live connection is open (#22): while it is not, POP's printers are
/// as BAM! last said, and changes wait for it to reconnect.
pub struct BamLiveHealth(pub tokio::sync::watch::Receiver<pop_integrations::sync::View>);

impl HealthCheck for BamLiveHealth {
    fn name(&self) -> &'static str {
        "bam_live"
    }

    fn check(&self) -> CheckFuture<'_> {
        let live = self.0.borrow().live;
        Box::pin(async move {
            if live {
                Ok(())
            } else {
                Err("BAM!'s live connection is down: printers are as BAM! last said".to_owned())
            }
        })
    }
}

/// The printer `bam_id` as BAM! has it now, in the category and in use, and
/// whether it is ready to run; or the answer to give instead.
/// Why `current` could not say: a status and a sentence, answered as an
/// error.
struct Unavailable(StatusCode, String);

impl IntoResponse for Unavailable {
    fn into_response(self) -> Response {
        error(self.0, self.1)
    }
}

async fn current(
    state: &AppState,
    bam_id: &str,
) -> Result<(String, Vec<PrinterProblem>), Unavailable> {
    let Some(bam) = state.bam.as_ref() else {
        return Err(Unavailable(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP has no BAM! to read printers from".to_owned(),
        ));
    };
    let assets = bam
        .client
        .assets(&bam.category, false)
        .await
        .map_err(|e| Unavailable(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let Some(a) = assets.iter().find(|a| a.asset.id == bam_id) else {
        return Err(Unavailable(
            StatusCode::NOT_FOUND,
            "no such printer in the BAM! category: it may have been archived or moved".to_owned(),
        ));
    };
    let held = Secrets::with_access_codes(&state.db)
        .await
        .map_err(|e| Unavailable(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    let problems = match printer_from(a, &bam.fields, &|id: &str| held.iter().any(|h| h == id)) {
        Ok(_) => vec![],
        Err(ps) => ps
            .into_iter()
            .map(|p| PrinterProblem {
                field: p.field,
                problem: p.problem,
                in_pop: p.in_pop,
            })
            .collect(),
    };
    Ok((a.asset.name.clone(), problems))
}

/// `POST /api/admin/printers/{bam_id}/add`: the tower runs it. Only a
/// printer with every field right: never half-added.
pub async fn add(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(bam_id): Path<String>,
) -> Response {
    let (name, problems) = match current(&state, &bam_id).await {
        Ok(c) => c,
        Err(r) => return r.into_response(),
    };
    if let Some(p) = problems.first() {
        return error(
            StatusCode::CONFLICT,
            format!(
                "{name} cannot be added until it is right: {}: {}",
                p.field, p.problem
            ),
        );
    }
    let summary = format!("{} added {name} to the tower", admin.name);
    let by = Actor::user(admin.id, &admin.name);
    match crate::tower::decide(&state.db, &bam_id, &name, TowerState::Added, &by, summary).await {
        Ok(()) => {
            state.decided.notify_one();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `POST /api/admin/printers/{bam_id}/decline`: not one for the tower.
pub async fn decline(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(bam_id): Path<String>,
) -> Response {
    let (name, _) = match current(&state, &bam_id).await {
        Ok(c) => c,
        Err(r) => return r.into_response(),
    };
    let summary = format!("{} said {name} is not one for the tower", admin.name);
    let by = Actor::user(admin.id, &admin.name);
    match crate::tower::decide(
        &state.db,
        &bam_id,
        &name,
        TowerState::Declined,
        &by,
        summary,
    )
    .await
    {
        Ok(()) => {
            state.decided.notify_one();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}
