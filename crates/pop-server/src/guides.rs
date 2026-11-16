//! Guides (#71): steps and yes/no flowcharts, stored in POP, in one format
//! that the kiosk, the phone and the public pages all read.
//!
//! A guide is an ordered set of steps from a start step; any step can be a
//! yes/no question branching to two others, and every path ends at a step
//! that says what to do. A guide that breaks that is never kept: [`check`]
//! names everything wrong with it, by step.
//!
//! Each guide has a stable slug (`stringing-on-petg`) for links; renaming one
//! keeps the old slug as a redirect. Only published guides are shown to
//! members or to the public (#94). Images are files under the data
//! directory's `files/`, beside the print files, so a backup takes them.

use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use base64::Engine;
use sqlx::SqlitePool;

use serde::{Deserialize, Serialize};

use crate::{
    api::{Guide, GuideProblem, GuideStatus, GuideStep, GuideSummary, Then},
    users::now,
};

/// What is kept in a guide's `body` column: all but its slug, title and
/// status, which have columns of their own.
#[derive(Serialize, Deserialize)]
struct StoredBody {
    summary: String,
    start: String,
    steps: Vec<GuideStep>,
}

fn body_json(g: &Guide) -> String {
    serde_json::to_string(&StoredBody {
        summary: g.summary.clone(),
        start: g.start.clone(),
        steps: g.steps.clone(),
    })
    .expect("a guide serializes")
}

/// Under the data directory's files/.
pub const IMAGES_DIR: &str = "guide-images";
/// The image types a guide may hold, and the most bytes one may be.
pub const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp"];
pub const IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;

/// Whether `s` is a slug: lowercase words joined by single hyphens.
pub fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.split('-').all(|w| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Everything wrong with a guide, or nothing. `images` are the ids of the
/// images it may use.
pub fn check(guide: &Guide, images: &BTreeSet<String>) -> Vec<GuideProblem> {
    let mut out = Vec::new();
    let mut whole = |p: String| {
        out.push(GuideProblem {
            step: None,
            problem: p,
        })
    };
    if !is_slug(&guide.slug) {
        whole(format!(
            "slug: {:?} is not a slug: lowercase letters and digits, words joined by hyphens, as in stringing-on-petg",
            guide.slug
        ));
    }
    if guide.title.trim().is_empty() {
        whole("title: give the guide a title".into());
    }
    let b = guide;
    if b.steps.is_empty() {
        whole("steps: a guide needs at least one step".into());
        return out;
    }
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    for (i, s) in b.steps.iter().enumerate() {
        let mut step = |p: String| {
            out.push(GuideProblem {
                step: Some(s.id.clone()),
                problem: p,
            })
        };
        if !is_slug(&s.id) {
            step(format!(
                "{:?} is not a step id: lowercase words joined by hyphens",
                s.id
            ));
        }
        if by_id.insert(&s.id, i).is_some() {
            step("another step has the same id".into());
        }
        if s.title.trim().is_empty() {
            step(match s.then {
                Then::Question { .. } => "the question is empty".into(),
                _ => "say what to do: the step's title is empty".into(),
            });
        }
        if let Some(img) = &s.image
            && !images.contains(img)
        {
            step(format!("image {img} is not one of this guide's"));
        }
    }
    let next_of = |t: &Then| -> Vec<(&'static str, String)> {
        match t {
            Then::Done => vec![],
            Then::Next { step } => vec![("next", step.clone())],
            Then::Question { yes, no } => vec![("yes", yes.clone()), ("no", no.clone())],
        }
    };
    let mut broken = false;
    for s in &b.steps {
        for (answer, to) in next_of(&s.then) {
            if !by_id.contains_key(to.as_str()) {
                broken = true;
                out.push(GuideProblem {
                    step: Some(s.id.clone()),
                    problem: format!("{answer} leads to {to:?}, which is not a step"),
                });
            }
        }
    }
    if !by_id.contains_key(b.start.as_str()) {
        out.push(GuideProblem {
            step: None,
            problem: format!("start: {:?} is not a step", b.start),
        });
        return out;
    }
    if broken {
        // Reachability means nothing over links that go nowhere.
        return out;
    }
    // Every step can be reached from the start.
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue = VecDeque::from([b.start.as_str()]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        for (_, to) in next_of(&b.steps[by_id[id]].then) {
            queue.push_back(b.steps[by_id[to.as_str()]].id.as_str());
        }
    }
    for s in &b.steps {
        if !seen.contains(s.id.as_str()) {
            out.push(GuideProblem {
                step: Some(s.id.clone()),
                problem: "no answer leads here from the start".into(),
            });
        }
    }
    // Every path ends at a step that says what to do: from every step, a
    // Done step can be reached. A loop back ("still stringing? start
    // again") is fine; a loop with no way out is not.
    let mut ends: HashSet<&str> = b
        .steps
        .iter()
        .filter(|s| s.then == Then::Done)
        .map(|s| s.id.as_str())
        .collect();
    loop {
        let more: Vec<&str> = b
            .steps
            .iter()
            .filter(|s| !ends.contains(s.id.as_str()))
            .filter(|s| {
                next_of(&s.then)
                    .iter()
                    .any(|(_, to)| ends.contains(to.as_str()))
            })
            .map(|s| s.id.as_str())
            .collect();
        if more.is_empty() {
            break;
        }
        ends.extend(more);
    }
    for s in &b.steps {
        if !ends.contains(s.id.as_str()) {
            out.push(GuideProblem {
                step: Some(s.id.clone()),
                problem:
                    "every answer from here loops without reaching a step that says what to do"
                        .into(),
            });
        }
    }
    out
}

fn status_str(s: GuideStatus) -> &'static str {
    match s {
        GuideStatus::Draft => "draft",
        GuideStatus::Published => "published",
    }
}

fn status_from(s: &str) -> GuideStatus {
    if s == "published" {
        GuideStatus::Published
    } else {
        GuideStatus::Draft
    }
}

/// Why a guide could not be kept.
#[derive(Debug)]
pub enum StoreError {
    /// Another guide has, or had, that slug.
    SlugTaken,
    NotFound,
    Invalid(Vec<GuideProblem>),
    Database(sqlx::Error),
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

/// The guide whose slug, or old slug, is `slug`: its id, and its slug now.
pub async fn find(db: &SqlitePool, slug: &str) -> Result<Option<(i64, String)>, sqlx::Error> {
    if let Some(found) = sqlx::query_as("SELECT id, slug FROM guides WHERE slug = ?")
        .bind(slug)
        .fetch_optional(db)
        .await?
    {
        return Ok(Some(found));
    }
    sqlx::query_as(
        "SELECT g.id, g.slug FROM guide_slugs o JOIN guides g ON g.id = o.guide_id WHERE o.slug = ?",
    )
    .bind(slug)
    .fetch_optional(db)
    .await
}

/// The ids of a guide's images.
pub async fn image_ids(db: &SqlitePool, guide_id: i64) -> Result<BTreeSet<String>, sqlx::Error> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM guide_images WHERE guide_id = ?")
        .bind(guide_id)
        .fetch_all(db)
        .await?;
    Ok(ids.into_iter().collect())
}

/// Every guide, by title.
pub async fn list(db: &SqlitePool, only_published: bool) -> Result<Vec<GuideSummary>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT slug, title, status, body, updated_at FROM guides ORDER BY title COLLATE NOCASE",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(slug, title, status, body, updated_at)| GuideSummary {
            slug,
            title,
            summary: serde_json::from_str::<StoredBody>(&body)
                .map(|b| b.summary)
                .unwrap_or_default(),
            status: status_from(&status),
            updated_at,
        })
        .filter(|g| !only_published || g.status == GuideStatus::Published)
        .collect())
}

/// The guide `slug` names exactly (not an old slug).
pub async fn get(db: &SqlitePool, slug: &str) -> Result<Option<Guide>, sqlx::Error> {
    let row: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT slug, title, status, body FROM guides WHERE slug = ?")
            .bind(slug)
            .fetch_optional(db)
            .await?;
    Ok(row.and_then(|(slug, title, status, body)| {
        let body: StoredBody = serde_json::from_str(&body).ok()?;
        Some(Guide {
            slug,
            title,
            status: status_from(&status),
            summary: body.summary,
            start: body.start,
            steps: body.steps,
        })
    }))
}

/// Whether `slug` is free for the guide `mine` (none for a new guide): no
/// other guide has it now or had it.
async fn free(db: &SqlitePool, slug: &str, mine: Option<i64>) -> Result<bool, sqlx::Error> {
    Ok(match find(db, slug).await? {
        None => true,
        Some((id, _)) => Some(id) == mine,
    })
}

/// Keep a new guide.
pub async fn create(db: &SqlitePool, guide: &Guide) -> Result<(), StoreError> {
    let problems = check(guide, &BTreeSet::new());
    if !problems.is_empty() {
        return Err(StoreError::Invalid(problems));
    }
    if !free(db, &guide.slug, None).await? {
        return Err(StoreError::SlugTaken);
    }
    let t = now();
    sqlx::query(
        "INSERT INTO guides (slug, title, status, body, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&guide.slug)
    .bind(guide.title.trim())
    .bind(status_str(guide.status))
    .bind(body_json(guide))
    .bind(t)
    .bind(t)
    .execute(db)
    .await?;
    Ok(())
}

/// Replace the guide at `slug` with `guide`, which may rename it: the old
/// slug is kept as a redirect.
pub async fn update(db: &SqlitePool, slug: &str, guide: &Guide) -> Result<(), StoreError> {
    let Some((id, current)) = find(db, slug).await? else {
        return Err(StoreError::NotFound);
    };
    if current != slug {
        // An old slug redirects readers; it is not an address to edit at.
        return Err(StoreError::NotFound);
    }
    let problems = check(guide, &image_ids(db, id).await?);
    if !problems.is_empty() {
        return Err(StoreError::Invalid(problems));
    }
    if !free(db, &guide.slug, Some(id)).await? {
        return Err(StoreError::SlugTaken);
    }
    let mut tx = db.begin().await?;
    if guide.slug != current {
        // The new slug may be one of its own old ones: it is current again.
        sqlx::query("DELETE FROM guide_slugs WHERE slug = ?")
            .bind(&guide.slug)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO guide_slugs (slug, guide_id) VALUES (?, ?)")
            .bind(&current)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "UPDATE guides SET slug = ?, title = ?, status = ?, body = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&guide.slug)
    .bind(guide.title.trim())
    .bind(status_str(guide.status))
    .bind(body_json(guide))
    .bind(now())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Why an image could not be kept.
#[derive(Debug)]
pub enum ImageError {
    Refused(String),
    Io(std::io::Error),
    Database(sqlx::Error),
}

/// Keep an image for the guide `guide_id`, under `files_dir`. Its id.
pub async fn save_image(
    db: &SqlitePool,
    files_dir: &Path,
    guide_id: i64,
    content_type: &str,
    data_base64: &str,
) -> Result<String, ImageError> {
    let content_type = content_type.trim().to_ascii_lowercase();
    if !IMAGE_TYPES.contains(&content_type.as_str()) {
        return Err(ImageError::Refused(format!(
            "content_type: {content_type:?} is not one of {}",
            IMAGE_TYPES.join(", ")
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.trim())
        .map_err(|e| ImageError::Refused(format!("data_base64: not base64: {e}")))?;
    if bytes.is_empty() || bytes.len() > IMAGE_MAX_BYTES {
        return Err(ImageError::Refused(format!(
            "data_base64: an image is 1 byte to {} MB; this is {} bytes",
            IMAGE_MAX_BYTES / 1024 / 1024,
            bytes.len()
        )));
    }
    if !looks_like(&content_type, &bytes) {
        return Err(ImageError::Refused(format!(
            "data_base64: these bytes are not {content_type}"
        )));
    }
    let id = hex::encode(rand::random::<[u8; 16]>());
    let dir = files_dir.join(IMAGES_DIR);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(ImageError::Io)?;
    let path = dir.join(&id);
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(ImageError::Io)?;
    let kept = sqlx::query(
        "INSERT INTO guide_images (id, guide_id, content_type, bytes, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(guide_id)
    .bind(&content_type)
    .bind(bytes.len() as i64)
    .bind(now())
    .execute(db)
    .await;
    if let Err(e) = kept {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ImageError::Database(e));
    }
    Ok(id)
}

/// Whether `bytes` start as `content_type`'s files do, so a file is served as
/// what it is.
fn looks_like(content_type: &str, bytes: &[u8]) -> bool {
    match content_type {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

/// Where an image's file is, and its type, if the guide `guide_id` has it.
pub async fn image(
    db: &SqlitePool,
    files_dir: &Path,
    guide_id: i64,
    id: &str,
) -> Result<Option<(PathBuf, String)>, sqlx::Error> {
    let ct: Option<String> =
        sqlx::query_scalar("SELECT content_type FROM guide_images WHERE id = ? AND guide_id = ?")
            .bind(id)
            .bind(guide_id)
            .fetch_optional(db)
            .await?;
    Ok(ct.map(|ct| (files_dir.join(IMAGES_DIR).join(id), ct)))
}

// ---- the admin API -----------------------------------------------------------

use axum::{
    Extension, Json,
    body::Body,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{
    AppState,
    api::{ApiError, AuditKind, GuideImage, GuideProblems, NewGuideImage, User},
    audit::{self, Actor, Event},
};

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

fn store_error(e: StoreError, slug: &str) -> Response {
    match e {
        StoreError::SlugTaken => error(
            StatusCode::CONFLICT,
            format!("slug: {slug:?} belongs to another guide, now or before a rename"),
        ),
        StoreError::NotFound => error(StatusCode::NOT_FOUND, "no guide has that slug"),
        StoreError::Invalid(problems) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(GuideProblems {
                error: format!(
                    "the guide cannot be kept: {}",
                    problems
                        .iter()
                        .map(|p| match &p.step {
                            Some(s) => format!("step {s}: {}", p.problem),
                            None => p.problem.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
                problems,
            }),
        )
            .into_response(),
        StoreError::Database(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

async fn record(
    state: &AppState,
    admin: &User,
    kind: AuditKind,
    summary: String,
) -> Result<(), String> {
    let mut conn = state.db.acquire().await.map_err(|e| e.to_string())?;
    audit::record(
        &mut conn,
        &Actor::user(admin.id, &admin.name),
        Event {
            kind,
            printer: None,
            subject: None,
            summary,
        },
    )
    .await
    .map_err(|e| e.to_string())
}

/// `GET /api/admin/guides`: every guide, drafts too.
pub async fn admin_list(State(state): State<AppState>) -> Response {
    match list(&state.db, false).await {
        Ok(gs) => Json(gs).into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `GET /api/admin/guides/{slug}`.
pub async fn admin_get(State(state): State<AppState>, UrlPath(slug): UrlPath<String>) -> Response {
    match get(&state.db, &slug).await {
        Ok(Some(g)) => Json(g).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "no guide has that slug"),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `POST /api/admin/guides`.
pub async fn admin_create(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    body: Result<Json<Guide>, JsonRejection>,
) -> Response {
    let Json(guide) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    if let Err(e) = create(&state.db, &guide).await {
        return store_error(e, &guide.slug);
    }
    let summary = format!(
        "{} wrote the guide {} ({})",
        admin.name,
        guide.title.trim(),
        guide.slug
    );
    if let Err(e) = record(&state, &admin, AuditKind::GuideCreated, summary).await {
        return error(StatusCode::SERVICE_UNAVAILABLE, e);
    }
    (StatusCode::CREATED, Json(guide)).into_response()
}

/// `PUT /api/admin/guides/{slug}`: replace it, perhaps renaming it.
pub async fn admin_update(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    UrlPath(slug): UrlPath<String>,
    body: Result<Json<Guide>, JsonRejection>,
) -> Response {
    let Json(guide) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let before = match get(&state.db, &slug).await {
        Ok(b) => b,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    if let Err(e) = update(&state.db, &slug, &guide).await {
        return store_error(e, &guide.slug);
    }
    let mut said = vec![];
    if guide.slug != slug {
        said.push(format!("renamed it from {slug}, which still leads to it"));
    }
    match (before.map(|b| b.status), guide.status) {
        (Some(GuideStatus::Draft), GuideStatus::Published) => said.push("published it".into()),
        (Some(GuideStatus::Published), GuideStatus::Draft) => {
            said.push("took it back to a draft".into())
        }
        _ => {}
    }
    let summary = if said.is_empty() {
        format!(
            "{} edited the guide {} ({})",
            admin.name,
            guide.title.trim(),
            guide.slug
        )
    } else {
        format!(
            "{} edited the guide {} ({}) and {}",
            admin.name,
            guide.title.trim(),
            guide.slug,
            said.join(" and ")
        )
    };
    if let Err(e) = record(&state, &admin, AuditKind::GuideEdited, summary).await {
        return error(StatusCode::SERVICE_UNAVAILABLE, e);
    }
    Json(guide).into_response()
}

/// `POST /api/admin/guides/{slug}/images`.
pub async fn admin_add_image(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    UrlPath(slug): UrlPath<String>,
    body: Result<Json<NewGuideImage>, JsonRejection>,
) -> Response {
    let Json(img) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let Some(files) = state.files_dir.clone() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "POP has nowhere to keep files",
        );
    };
    let id = match find(&state.db, &slug).await {
        Ok(Some((id, current))) if current == slug => id,
        Ok(_) => return error(StatusCode::NOT_FOUND, "no guide has that slug"),
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let image = match save_image(&state.db, &files, id, &img.content_type, &img.data_base64).await {
        Ok(i) => i,
        Err(ImageError::Refused(why)) => return error(StatusCode::BAD_REQUEST, why),
        Err(ImageError::Io(e)) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
        Err(ImageError::Database(e)) => {
            return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string());
        }
    };
    let summary = format!("{} added an image to the guide {slug}", admin.name);
    if let Err(e) = record(&state, &admin, AuditKind::GuideEdited, summary).await {
        return error(StatusCode::SERVICE_UNAVAILABLE, e);
    }
    (StatusCode::CREATED, Json(GuideImage { id: image })).into_response()
}

/// `GET /api/admin/guides/{slug}/images/{id}`: a draft's images too.
pub async fn admin_image(
    State(state): State<AppState>,
    UrlPath((slug, image_id)): UrlPath<(String, String)>,
) -> Response {
    let Some(files) = state.files_dir.clone() else {
        return error(StatusCode::NOT_FOUND, "no such image");
    };
    let id = match find(&state.db, &slug).await {
        Ok(Some((id, _))) => id,
        Ok(None) => return error(StatusCode::NOT_FOUND, "no such image"),
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    serve_image(&state.db, &files, id, &image_id).await
}

/// An image's bytes, as its type, or 404.
pub async fn serve_image(db: &SqlitePool, files: &Path, guide_id: i64, image_id: &str) -> Response {
    match image(db, files, guide_id, image_id).await {
        Ok(Some((path, ct))) => match tokio::fs::read(&path).await {
            Ok(bytes) => (
                [
                    (header::CONTENT_TYPE, ct),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
                ],
                Body::from(bytes),
            )
                .into_response(),
            Err(e) => {
                tracing::error!(
                    "guide image {image_id} is in the database but its file is not: {e}"
                );
                error(StatusCode::NOT_FOUND, "no such image")
            }
        },
        Ok(None) => error(StatusCode::NOT_FOUND, "no such image"),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}
