//! The web app, served by the same binary as the API (ADR 0001).
//!
//! Which file a path gets is decided by [`spa`], over any [`Assets`], so the
//! rules can be tested without a built app. The release binary serves
//! [`Embedded`]: `web/dist`, built by `tools/web/build.sh`, compiled in.

use std::borrow::Cow;

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{AppState, api::ApiError};

/// A file of the web app.
pub struct Asset {
    pub bytes: Cow<'static, [u8]>,
    pub mime: String,
}

/// Somewhere the web app's files come from.
pub trait Assets: Send + Sync {
    /// The file at `path`, relative to the app's root (`index.html`,
    /// `assets/app-1a2b.js`).
    fn get(&self, path: &str) -> Option<Asset>;
}

/// The web app as built into the binary. Empty if `web/dist` did not exist
/// when the binary was compiled; the server then says so on every page.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
pub struct Embedded;

impl Assets for Embedded {
    fn get(&self, path: &str) -> Option<Asset> {
        <Embedded as rust_embed::RustEmbed>::get(path).map(|f| Asset {
            mime: f.metadata.mimetype().to_owned(),
            bytes: f.data,
        })
    }
}

impl Embedded {
    /// Whether the app was built into this binary.
    pub fn present() -> bool {
        <Embedded as rust_embed::RustEmbed>::get("index.html").is_some()
    }
}

/// The fallback route: everything that is not an API route.
///
/// - Anything under `/api` that no route matched is a JSON 404. An API client
///   must never be handed the app's HTML in place of an error.
/// - A path that names a file the app has is that file.
/// - A path that looks like a file (it has an extension) but is not one is a
///   404, so a missing script fails loudly rather than as HTML.
/// - Anything else is a screen of the app (`/kiosk`, `/jobs/42`), which the
///   app routes itself: it gets `index.html`.
pub async fn spa(State(state): State<AppState>, req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!("no such endpoint: /{path}"),
            }),
        )
            .into_response();
    }
    let assets = state.assets.as_ref();
    if !path.is_empty()
        && let Some(asset) = assets.get(path)
    {
        // Vite names everything under assets/ by its content hash, so those
        // never change under a name. Anything else may.
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return file(asset, cache);
    }
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    match assets.get("index.html") {
        Some(index) => {
            let mut res = file(index, "no-cache");
            // Only the public guides may be indexed, and only when that is
            // allowed (#94); never the kiosk, the phone or the admin desk.
            let guides = path == "guides" || path.starts_with("guides/");
            if !(guides && state.guides.indexable) {
                res.headers_mut()
                    .insert("x-robots-tag", HeaderValue::from_static("noindex"));
            }
            res
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "This POP was built without its web app. Run tools/web/build.sh, then build the server again.",
        )
            .into_response(),
    }
}

fn file(asset: Asset, cache: &'static str) -> Response {
    let mut res = asset.bytes.into_owned().into_response();
    let headers = res.headers_mut();
    if let Ok(mime) = HeaderValue::from_str(&asset.mime) {
        headers.insert(header::CONTENT_TYPE, mime);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    res
}
