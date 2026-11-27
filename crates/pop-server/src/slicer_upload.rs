//! The slicer's upload (#37): `POST /api/files/local`, as OctoPrint answers
//! it, so the tower can be added as a physical printer in PrusaSlicer,
//! SuperSlicer and OrcaSlicer. The slicer's personal API key (#13), in
//! `X-Api-Key`, says whose print it is.
//!
//! The request is a multipart form: the file as `file`, with `print`
//! ("true" for "Upload and print"), `path`, and from OrcaSlicer, for a
//! `.gcode.3mf`, `plateindex`. Upload and print is taken as upload: the print
//! waits for its owner to tap in at the tower (#45). `path` names an
//! OctoPrint folder, which POP has none of, and is ignored.
//!
//! The file streams to `uploads/` under the data directory, beside the files
//! directory so a half-arrived upload is never in a backup, and is refused as
//! soon as it passes `[prints].max_file_mb`. Then it is stored as any print
//! is (crate::prints).

use std::path::{Path, PathBuf};

use axum::{
    Extension, Json,
    extract::{Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use tokio::io::AsyncWriteExt;

use crate::{
    AppState,
    api::{ApiError, User},
    prints::{self, StoreError},
};

/// Where uploads arrive, under the data directory.
pub const UPLOADS_DIR: &str = "uploads";
/// The most a form field other than the file may hold.
const FIELD_BYTES: usize = 1024;
/// The most fields a form may have: a slicer sends four or five.
const FIELDS: usize = 32;

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

/// An upload being written, deleted unless it is kept.
struct Arriving(Option<PathBuf>);

impl Drop for Arriving {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// `POST /api/files/local`.
pub async fn upload(
    State(state): State<AppState>,
    Extension(me): Extension<User>,
    mut form: Multipart,
) -> Response {
    let Some(files_dir) = state.files_dir.clone() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "this POP keeps no files");
    };
    let limit = state.prints.max_file_bytes();
    let dir = files_dir
        .parent()
        .map_or_else(|| PathBuf::from(UPLOADS_DIR), |d| d.join(UPLOADS_DIR));
    let mut file: Option<(String, Arriving)> = None;
    let mut fields = 0;
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    format!("the upload is not a form POP reads: {e}"),
                );
            }
        };
        fields += 1;
        if fields > FIELDS {
            return error(StatusCode::BAD_REQUEST, "the upload has too many fields");
        }
        if field.name() == Some("file") {
            if file.is_some() {
                return error(StatusCode::BAD_REQUEST, "send one file at a time");
            }
            let name = field.file_name().unwrap_or_default().to_owned();
            match receive(field, &dir, limit).await {
                Ok(arriving) => file = Some((name, arriving)),
                Err((code, why)) => return error(code, why),
            }
        } else {
            // print, select, path, plateindex and anything else: read, so
            // the form is read to its end, and bounded.
            let mut field = field;
            let mut n = 0;
            loop {
                match field.chunk().await {
                    Ok(Some(c)) => {
                        n += c.len();
                        if n > FIELD_BYTES {
                            return error(StatusCode::BAD_REQUEST, "a form field is too long");
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            format!("the upload is not a form POP reads: {e}"),
                        );
                    }
                }
            }
        }
    }
    let Some((name, arriving)) = file else {
        return error(StatusCode::BAD_REQUEST, "the upload has no file in it");
    };
    let path = arriving.0.clone().expect("an arriving upload has its path");
    match prints::store(&state.db, &files_dir, &state.prints, &me, &name, &path).await {
        // Stored, the upload was moved into place, so dropping `arriving`
        // finds nothing to delete; refused, it deletes it.
        Ok(print) => {
            drop(arriving);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "done": true,
                    "files": {"local": {"name": print.filename, "origin": "local"}},
                    // It waits for its owner at the tower, whatever was asked.
                    "effectiveSelect": false,
                    "effectivePrint": false,
                })),
            )
                .into_response()
        }
        Err(StoreError::Refused(why)) => error(StatusCode::BAD_REQUEST, why),
        Err(StoreError::Full(why)) => error(StatusCode::CONFLICT, why),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// Write the file field to `dir`, refusing it one byte past `limit`.
async fn receive(
    mut field: axum::extract::multipart::Field<'_>,
    dir: &Path,
    limit: u64,
) -> Result<Arriving, (StatusCode, String)> {
    let unavailable = |e: std::io::Error| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("the upload could not be kept: {e}"),
        )
    };
    tokio::fs::create_dir_all(dir).await.map_err(unavailable)?;
    let path = dir.join(format!("{}.part", hex::encode(rand::random::<[u8; 16]>())));
    let arriving = Arriving(Some(path.clone()));
    let mut out = tokio::fs::File::create(&path).await.map_err(unavailable)?;
    let mut n: u64 = 0;
    loop {
        match field.chunk().await {
            Ok(Some(c)) => {
                n += c.len() as u64;
                if n > limit {
                    return Err((
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!("a print may be at most {} MB", limit / 1024 / 1024),
                    ));
                }
                out.write_all(&c).await.map_err(unavailable)?;
            }
            Ok(None) => break,
            Err(e) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("the upload did not arrive whole: {e}"),
                ));
            }
        }
    }
    out.flush().await.map_err(unavailable)?;
    Ok(arriving)
}
