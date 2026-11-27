//! Prints waiting to be sent (#40): each member's uploads, kept on disk beside
//! the database until they are sent, deleted, or expire.
//!
//! A print's file is `files/prints/<id>`, and each plate's picture, when its
//! slicer drew one, `files/prints/<id>.plate-<n>.png`; both are in a backup
//! (crate::backup). The database row holds what the slicer wrote into the
//! file (pop_gcode), read once, when it is stored.
//!
//! A member sees, and deletes, only their own prints. Admins see and delete
//! everyone's. Someone else's print is not found, as one that does not exist.
//!
//! Limits (config::Prints): the largest file, all of one member's waiting
//! prints together, and how many they may have waiting. A print unsent after
//! `keep_days` expires: its files are deleted, and its row stays, listed to
//! its owner as expired, until they dismiss it.

use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use axum::{
    Extension, Json,
    body::Body,
    extract::{Path as UrlPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::task::JoinHandle;

use crate::{
    AppState,
    api::{
        ApiError, AuditKind, Print, PrintFilament, PrintFormat, PrintOwner, PrintPlate, Role, User,
    },
    audit::{self, Actor, Event},
    config,
    users::now,
};

/// Under the files directory.
pub const PRINTS_DIR: &str = "prints";
/// How often waiting prints are checked for expiry.
pub const SWEEP_EVERY: Duration = Duration::from_secs(10 * 60);
/// The largest picture of a plate kept.
const THUMBNAIL_BYTES: u64 = 1 << 20;
/// How much G-code a `.gcode.3mf` may hold, decompressed, for each byte of
/// the archive: deflate shrinks G-code about fivefold, so this is room to
/// spare and still a bound.
const INFLATION: u64 = 16;

/// Why a print was not stored.
#[derive(Debug)]
pub enum StoreError {
    /// Not a print POP keeps, in words for whoever uploaded it.
    Refused(String),
    /// Within what POP keeps, past what this member may have waiting.
    Full(String),
    Io(std::io::Error),
    Database(sqlx::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(why) | Self::Full(why) => f.write_str(why),
            Self::Io(e) => write!(f, "the print could not be kept: {e}"),
            Self::Database(e) => write!(f, "the print could not be recorded: {e}"),
        }
    }
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// The name to show for an upload: its last path component, trimmed, at most
/// 255 characters, and what kind of print its ending says it is.
fn named(filename: &str) -> Result<(String, PrintFormat), StoreError> {
    let base = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    let base: String = base.chars().filter(|c| !c.is_control()).collect();
    let lower = base.to_ascii_lowercase();
    let (format, ending) = if lower.ends_with(".gcode.3mf") {
        (PrintFormat::Gcode3mf, ".gcode.3mf".len())
    } else if lower.ends_with(".gcode") {
        (PrintFormat::Gcode, ".gcode".len())
    } else {
        return Err(StoreError::Refused(format!(
            "POP keeps sliced prints, .gcode and .gcode.3mf files: {base:?} is neither"
        )));
    };
    // The ending is ASCII, so it is the last `ending` bytes.
    let (stem, ending) = base.split_at(base.len() - ending);
    if stem.trim().is_empty() {
        return Err(StoreError::Refused("the file has no name".into()));
    }
    // At most 255 characters, the ending kept.
    let stem: String = stem.chars().take(255 - ending.len()).collect();
    Ok((format!("{stem}{ending}"), format))
}

/// A plate as read, with its picture if the slicer drew one.
type ReadPlate = (PrintPlate, Option<Vec<u8>>);

/// What the slicer wrote into a print, and the file's SHA-256, read from
/// `file` in one pass for G-code and two for an archive.
fn read_print(
    file: &Path,
    format: PrintFormat,
    bytes: u64,
) -> Result<(Vec<ReadPlate>, String), StoreError> {
    let mut hash = Sha256::new();
    let mut f = std::fs::File::open(file)?;
    let mut buf = vec![0; 256 * 1024];
    let mut scanner = pop_gcode::Scanner::new();
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        if format == PrintFormat::Gcode {
            scanner.push(&buf[..n]);
        }
    }
    let sha256 = hex::encode(hash.finalize());
    let plates = match format {
        PrintFormat::Gcode => vec![(plate(1, scanner.finish(), false), None)],
        PrintFormat::Gcode3mf => {
            let limits = pop_gcode::Limits {
                gcode_bytes: bytes.saturating_mul(INFLATION),
                thumbnail_bytes: THUMBNAIL_BYTES,
            };
            pop_gcode::read_3mf(std::fs::File::open(file)?, &limits)
                .map_err(|e| StoreError::Refused(e.to_string()))?
                .into_iter()
                .map(|p| {
                    let has = p.thumbnail.is_some();
                    (plate(p.index, p.metadata, has), p.thumbnail)
                })
                .collect()
        }
    };
    Ok((plates, sha256))
}

fn plate(index: u32, m: pop_gcode::Metadata, has_thumbnail: bool) -> PrintPlate {
    use pop_gcode::SlicerKind as K;
    PrintPlate {
        index,
        slicer: m.slicer.map(|s| {
            let name = match s.kind {
                K::PrusaSlicer => "PrusaSlicer".to_owned(),
                K::SuperSlicer => "SuperSlicer".to_owned(),
                K::OrcaSlicer => "OrcaSlicer".to_owned(),
                K::BambuStudio => "Bambu Studio".to_owned(),
                K::Cura => "Cura".to_owned(),
                K::Other(name) => name,
            };
            match s.version {
                Some(v) => format!("{name} {v}"),
                None => name,
            }
        }),
        printer_model: m.printer_model,
        nozzles_mm: m.nozzles_mm,
        estimated_seconds: m.estimated_seconds,
        filaments: m
            .filaments
            .into_iter()
            .map(|f| PrintFilament {
                slot: f.slot as u32,
                material: f.material,
                color: f.color,
                grams: f.grams,
            })
            .collect(),
        total_grams: m.total_grams,
        has_thumbnail,
    }
}

fn file_of(files_dir: &Path, id: &str) -> PathBuf {
    files_dir.join(PRINTS_DIR).join(id)
}

fn thumbnail_of(files_dir: &Path, id: &str, plate: u32) -> PathBuf {
    files_dir
        .join(PRINTS_DIR)
        .join(format!("{id}.plate-{plate}.png"))
}

/// Delete a print's files: its own, and every plate's picture.
async fn remove_files(files_dir: &Path, id: &str, plates: &[PrintPlate]) {
    let _ = tokio::fs::remove_file(file_of(files_dir, id)).await;
    for p in plates.iter().filter(|p| p.has_thumbnail) {
        let _ = tokio::fs::remove_file(thumbnail_of(files_dir, id, p.index)).await;
    }
}

/// Keep the upload at `file`, named `filename` by its uploader, as a print
/// `owner` waits to send. `file` is moved, not copied: it must be on the
/// files directory's filesystem, and is gone once this returns `Ok`.
pub async fn store(
    db: &SqlitePool,
    files_dir: &Path,
    limits: &config::Prints,
    owner: &User,
    filename: &str,
    file: &Path,
) -> Result<Print, StoreError> {
    let (filename, format) = named(filename)?;
    let bytes = tokio::fs::metadata(file).await?.len();
    if bytes == 0 {
        return Err(StoreError::Refused(format!("{filename} is empty")));
    }
    if bytes > limits.max_file_bytes() {
        return Err(StoreError::Refused(format!(
            "{filename} is {} MB; a print may be at most {} MB",
            bytes.div_ceil(1024 * 1024),
            limits.max_file_mb
        )));
    }
    let path = file.to_owned();
    let (plates, sha256) = tokio::task::spawn_blocking(move || read_print(&path, format, bytes))
        .await
        .map_err(|e| StoreError::Io(std::io::Error::other(e)))??;

    let id = hex::encode(rand::random::<[u8; 16]>());
    tokio::fs::create_dir_all(files_dir.join(PRINTS_DIR)).await?;
    tokio::fs::rename(file, file_of(files_dir, &id)).await?;
    let mut wrote = Vec::new();
    for (p, png) in &plates {
        if let Some(png) = png {
            if let Err(e) = tokio::fs::write(thumbnail_of(files_dir, &id, p.index), png).await {
                let kept: Vec<PrintPlate> = wrote.clone();
                remove_files(files_dir, &id, &kept).await;
                return Err(e.into());
            }
            wrote.push(p.clone());
        }
    }
    let plates: Vec<PrintPlate> = plates.into_iter().map(|(p, _)| p).collect();
    let created_at = now();
    let print = Print {
        id: id.clone(),
        owner: PrintOwner {
            id: owner.id,
            name: owner.name.clone(),
        },
        filename,
        format,
        bytes,
        created_at,
        expires_at: created_at + limits.keep_secs(),
        expired_at: None,
        plates,
    };
    match record(db, limits, &print, &sha256, owner).await {
        Ok(true) => Ok(print),
        Ok(false) => {
            remove_files(files_dir, &id, &print.plates).await;
            Err(StoreError::Full(format!(
                "you have as many prints waiting as you may: at most {} prints, \
                 {} MB together. Send or delete one first",
                limits.max_waiting, limits.member_mb
            )))
        }
        Err(e) => {
            remove_files(files_dir, &id, &print.plates).await;
            Err(e.into())
        }
    }
}

/// The row and its audit entry, if the owner's waiting prints leave room:
/// one statement checks and inserts, so two uploads at once cannot both take
/// the last place.
async fn record(
    db: &SqlitePool,
    limits: &config::Prints,
    print: &Print,
    sha256: &str,
    owner: &User,
) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let format = match print.format {
        PrintFormat::Gcode => "gcode",
        PrintFormat::Gcode3mf => "gcode.3mf",
    };
    let plates = serde_json::to_string(&print.plates).expect("plates serialize");
    let done = sqlx::query(
        "INSERT INTO prints (id, user_id, filename, format, bytes, sha256, plates, created_at, expires_at)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
          WHERE (SELECT COUNT(*) FROM prints WHERE user_id = ?2 AND expired_at IS NULL) < ?10
            AND (SELECT COALESCE(SUM(bytes), 0) FROM prints WHERE user_id = ?2 AND expired_at IS NULL) + ?5 <= ?11",
    )
    .bind(&print.id)
    .bind(owner.id)
    .bind(&print.filename)
    .bind(format)
    .bind(print.bytes as i64)
    .bind(sha256)
    .bind(&plates)
    .bind(print.created_at)
    .bind(print.expires_at)
    .bind(i64::from(limits.max_waiting))
    .bind(limits.member_bytes() as i64)
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    audit::record(
        &mut tx,
        &Actor::user(owner.id, &owner.name),
        Event {
            kind: AuditKind::PrintStored,
            printer: None,
            subject: Some((owner.id, &owner.name)),
            summary: format!("{} stored the print {:?}", owner.name, print.filename),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

// ---- reading them back --------------------------------------------------------

type Row = (
    String,
    i64,
    String,
    String,
    String,
    i64,
    String,
    i64,
    i64,
    Option<i64>,
);

const SELECT: &str = "SELECT p.id, p.user_id, u.name, p.filename, p.format, p.bytes, p.plates,
        p.created_at, p.expires_at, p.expired_at
   FROM prints p JOIN users u ON u.id = p.user_id";

fn print_of(r: Row) -> Print {
    let (id, user_id, name, filename, format, bytes, plates, created_at, expires_at, expired_at) =
        r;
    Print {
        id,
        owner: PrintOwner { id: user_id, name },
        filename,
        format: if format == "gcode.3mf" {
            PrintFormat::Gcode3mf
        } else {
            PrintFormat::Gcode
        },
        bytes: bytes as u64,
        created_at,
        expires_at,
        expired_at,
        // Written by this module from the same type; a row that does not read
        // shows no plates rather than hiding the print.
        plates: serde_json::from_str(&plates).unwrap_or_default(),
    }
}

/// The print `id`, if `me` may see it: their own, or anyone's to an admin.
pub async fn visible(db: &SqlitePool, me: &User, id: &str) -> Result<Option<Print>, sqlx::Error> {
    let row: Option<Row> =
        sqlx::query_as(&format!("{SELECT} WHERE p.id = ? AND (p.user_id = ? OR ?)"))
            .bind(id)
            .bind(me.id)
            .bind(me.role == Role::Admin)
            .fetch_optional(db)
            .await?;
    Ok(row.map(print_of))
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "you have no such print")
}

/// `GET /api/prints`: the signed-in person's own prints, waiting and
/// expired, newest first.
pub async fn mine(State(state): State<AppState>, Extension(me): Extension<User>) -> Response {
    let rows: Result<Vec<Row>, _> = sqlx::query_as(&format!(
        "{SELECT} WHERE p.user_id = ? ORDER BY p.created_at DESC, p.id"
    ))
    .bind(me.id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(rows.into_iter().map(print_of).collect::<Vec<_>>()).into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `GET /api/admin/prints`: everyone's, newest first.
pub async fn everyone(State(state): State<AppState>) -> Response {
    let rows: Result<Vec<Row>, _> =
        sqlx::query_as(&format!("{SELECT} ORDER BY p.created_at DESC, p.id"))
            .fetch_all(&state.db)
            .await;
    match rows {
        Ok(rows) => Json(rows.into_iter().map(print_of).collect::<Vec<_>>()).into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `GET /api/prints/{id}`.
pub async fn one(
    State(state): State<AppState>,
    Extension(me): Extension<User>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    match visible(&state.db, &me, &id).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => not_found(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `GET /api/prints/{id}/plates/{index}/thumbnail`: the slicer's picture of
/// a plate, while the print waits.
pub async fn thumbnail(
    State(state): State<AppState>,
    Extension(me): Extension<User>,
    UrlPath((id, index)): UrlPath<(String, u32)>,
) -> Response {
    let Some(files_dir) = state.files_dir.clone() else {
        return not_found();
    };
    let print = match visible(&state.db, &me, &id).await {
        Ok(Some(p)) => p,
        Ok(None) => return not_found(),
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    };
    let drawn = print.expired_at.is_none()
        && print
            .plates
            .iter()
            .any(|p| p.index == index && p.has_thumbnail);
    if !drawn {
        return error(StatusCode::NOT_FOUND, "that plate has no picture");
    }
    match tokio::fs::read(thumbnail_of(&files_dir, &print.id, index)).await {
        Ok(png) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                // Served as the PNG it was checked to start as, and as
                // nothing a browser might guess instead.
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            Body::from(png),
        )
            .into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `DELETE /api/prints/{id}`: delete a waiting print, its owner's or, by an
/// admin, anyone's; or dismiss the notice of one that expired.
pub async fn delete(
    State(state): State<AppState>,
    Extension(me): Extension<User>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    let Some(files_dir) = state.files_dir.clone() else {
        return not_found();
    };
    let done = async {
        let mut tx = state.db.begin().await?;
        let row: Option<Row> =
            sqlx::query_as(&format!("{SELECT} WHERE p.id = ? AND (p.user_id = ? OR ?)"))
                .bind(&id)
                .bind(me.id)
                .bind(me.role == Role::Admin)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(print) = row.map(print_of) else {
            return Ok(None);
        };
        sqlx::query("DELETE FROM prints WHERE id = ?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        if print.expired_at.is_none() {
            let whose = if print.owner.id == me.id {
                "their".to_owned()
            } else {
                format!("{}'s", print.owner.name)
            };
            audit::record(
                &mut tx,
                &Actor::user(me.id, &me.name),
                Event {
                    kind: AuditKind::PrintDeleted,
                    printer: None,
                    subject: Some((print.owner.id, &print.owner.name)),
                    summary: format!("{} deleted {whose} print {:?}", me.name, print.filename),
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(Some(print))
    }
    .await;
    match done {
        Ok(Some(print)) => {
            // After the commit: a file left behind by a failure here wastes
            // space; a row left pointing at no file would be a print that
            // cannot be sent.
            remove_files(&files_dir, &print.id, &print.plates).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => not_found(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

// ---- expiry -----------------------------------------------------------------

/// Expire every print still waiting at `at` past its time: delete its files,
/// and keep its row as the notice its owner sees. Returns how many.
pub async fn expire(db: &SqlitePool, files_dir: &Path, at: i64) -> Result<usize, sqlx::Error> {
    let due: Vec<Row> = sqlx::query_as(&format!(
        "{SELECT} WHERE p.expired_at IS NULL AND p.expires_at <= ? ORDER BY p.expires_at"
    ))
    .bind(at)
    .fetch_all(db)
    .await?;
    let mut n = 0;
    for print in due.into_iter().map(print_of) {
        let mut tx = db.begin().await?;
        let marked =
            sqlx::query("UPDATE prints SET expired_at = ? WHERE id = ? AND expired_at IS NULL")
                .bind(at)
                .bind(&print.id)
                .execute(&mut *tx)
                .await?;
        if marked.rows_affected() == 0 {
            // Deleted or expired meanwhile.
            continue;
        }
        audit::record(
            &mut tx,
            &Actor::expiry(),
            Event {
                kind: AuditKind::PrintExpired,
                printer: None,
                subject: Some((print.owner.id, &print.owner.name)),
                summary: format!(
                    "{}'s print {:?} waited unsent past its time and was deleted",
                    print.owner.name, print.filename
                ),
            },
        )
        .await?;
        tx.commit().await?;
        remove_files(files_dir, &print.id, &print.plates).await;
        n += 1;
    }
    Ok(n)
}

/// Expire prints now, and every [`SWEEP_EVERY`] after.
pub fn spawn(db: SqlitePool, files_dir: PathBuf) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match expire(&db, &files_dir, now()).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(expired = n, "deleted prints that waited unsent too long"),
                Err(e) => tracing::warn!(error = %e, "could not expire waiting prints"),
            }
            tokio::time::sleep(SWEEP_EVERY).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_its_last_part_and_says_what_it_is() {
        assert_eq!(
            named("C:\\Users\\ada\\Benchy.GCODE").unwrap(),
            ("Benchy.GCODE".into(), PrintFormat::Gcode)
        );
        assert_eq!(
            named("/tmp/plate one.gcode.3mf").unwrap(),
            ("plate one.gcode.3mf".into(), PrintFormat::Gcode3mf)
        );
        assert_eq!(
            named(" bed\u{7}.gcode ").unwrap().0,
            "bed.gcode",
            "no control characters"
        );
        for bad in ["model.3mf", "model.stl", "notes.txt", ".gcode", "dir/", ""] {
            assert!(matches!(named(bad), Err(StoreError::Refused(_))), "{bad:?}");
        }
        let long = format!("{}.gcode.3mf", "é".repeat(400));
        let (name, format) = named(&long).unwrap();
        assert_eq!(name.chars().count(), 255);
        assert!(name.ends_with("é.gcode.3mf"), "the ending is kept");
        assert_eq!(format, PrintFormat::Gcode3mf);
    }
}
