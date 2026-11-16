//! Public guides (#94): published guides, readable by anyone from anywhere,
//! without signing in; drafts and their images are not there at all.

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use base64::Engine;
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    api::{Guide, GuideStatus, GuideStep, Then},
    assets::Embedded,
    config::Guides,
    db, guides, router,
};
use serde_json::Value;
use sqlx::SqlitePool;
use tower::ServiceExt;

/// From outside the space, signed out: a stranger.
const STRANGER: &str = "203.0.113.9:4000";
const PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";

fn guide(slug: &str, title: &str, status: GuideStatus) -> Guide {
    Guide {
        slug: slug.into(),
        title: title.into(),
        status,
        summary: format!("About {title}."),
        start: "do-it".into(),
        steps: vec![GuideStep {
            id: "do-it".into(),
            title: "Do the thing.".into(),
            body: String::new(),
            image: None,
            then: Then::Done,
        }],
    }
}

struct Pop {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    files: std::path::PathBuf,
}

async fn pop(settings: Guides) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let files = dir.path().join("files");
    let app = router(
        AppState::new(db.clone(), Arc::new(Embedded))
            .with_files_dir(files.clone())
            .with_guides(settings),
    );
    Pop {
        _dir: dir,
        db,
        app,
        files,
    }
}

impl Pop {
    async fn get(&self, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut req = Request::get(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let mut req = req.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(STRANGER.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let (code, h) = (res.status(), res.headers().clone());
        (
            code,
            h,
            res.into_body().collect().await.unwrap().to_bytes().to_vec(),
        )
    }

    async fn json(&self, uri: &str) -> (StatusCode, Value) {
        let (c, _, b) = self.get(uri, &[]).await;
        (c, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    async fn image(&self, slug: &str) -> String {
        let (id, _) = guides::find(&self.db, slug).await.unwrap().unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(PNG);
        match guides::save_image(&self.db, &self.files, id, "image/png", &data).await {
            Ok(i) => i,
            Err(_) => panic!("the image was not kept"),
        }
    }
}

/// The issue's "done when".
#[tokio::test]
async fn a_stranger_reads_a_published_guide_and_finds_no_draft_or_its_image() {
    let pop = pop(Guides::default()).await;
    guides::create(
        &pop.db,
        &guide("stringing", "Stringing", GuideStatus::Published),
    )
    .await
    .unwrap();
    guides::create(&pop.db, &guide("warping", "Warping", GuideStatus::Draft))
        .await
        .unwrap();
    let draft_image = pop.image("warping").await;
    let published_image = pop.image("stringing").await;

    let (c, g) = pop.json("/api/public/guides/stringing").await;
    assert_eq!(c, StatusCode::OK);
    assert_eq!(g["title"], "Stringing");
    assert_eq!(g["steps"][0]["title"], "Do the thing.");
    assert!(
        g.get("status").is_none(),
        "nothing about drafts in a public answer"
    );

    let (c, _) = pop.json("/api/public/guides/warping").await;
    assert_eq!(c, StatusCode::NOT_FOUND);
    let (c, _, _) = pop
        .get(
            &format!("/api/public/guides/warping/images/{draft_image}"),
            &[],
        )
        .await;
    assert_eq!(c, StatusCode::NOT_FOUND);

    let (c, h, b) = pop
        .get(
            &format!("/api/public/guides/stringing/images/{published_image}"),
            &[],
        )
        .await;
    assert_eq!(c, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "image/png");
    assert_eq!(b, PNG);
    // A draft's image is not reachable through a published guide either.
    let (c, _, _) = pop
        .get(
            &format!("/api/public/guides/stringing/images/{draft_image}"),
            &[],
        )
        .await;
    assert_eq!(c, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_public_list_has_only_published_guides() {
    let pop = pop(Guides::default()).await;
    guides::create(
        &pop.db,
        &guide("stringing", "Stringing", GuideStatus::Published),
    )
    .await
    .unwrap();
    guides::create(&pop.db, &guide("warping", "Warping", GuideStatus::Draft))
        .await
        .unwrap();
    guides::create(
        &pop.db,
        &guide("clogs", "Clogged nozzles", GuideStatus::Published),
    )
    .await
    .unwrap();
    let (c, list) = pop.json("/api/public/guides").await;
    assert_eq!(c, StatusCode::OK);
    let slugs: Vec<&str> = list["guides"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["clogs", "stringing"]);
}

#[tokio::test]
async fn an_old_slug_redirects_to_the_guide_as_it_is_now() {
    let pop = pop(Guides::default()).await;
    guides::create(
        &pop.db,
        &guide("stringing-on-petg", "Stringing", GuideStatus::Published),
    )
    .await
    .unwrap();
    let image = pop.image("stringing-on-petg").await;
    let mut renamed = guide("stringing", "Stringing", GuideStatus::Published);
    renamed.steps[0].image = Some(image.clone());
    guides::update(&pop.db, "stringing-on-petg", &renamed)
        .await
        .unwrap();
    let (c, h, _) = pop.get("/api/public/guides/stringing-on-petg", &[]).await;
    assert_eq!(c, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(h[header::LOCATION], "/api/public/guides/stringing");
    let (c, h, _) = pop
        .get(
            &format!("/api/public/guides/stringing-on-petg/images/{image}"),
            &[],
        )
        .await;
    assert_eq!(c, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        h[header::LOCATION],
        format!("/api/public/guides/stringing/images/{image}")
    );
    // Taken back to a draft, even its old slug leads nowhere.
    let mut draft = renamed.clone();
    draft.status = GuideStatus::Draft;
    guides::update(&pop.db, "stringing", &draft).await.unwrap();
    let (c, _) = pop.json("/api/public/guides/stringing-on-petg").await;
    assert_eq!(c, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_etag_makes_asking_again_cheap_and_changes_with_the_guide() {
    let pop = pop(Guides::default()).await;
    guides::create(
        &pop.db,
        &guide("stringing", "Stringing", GuideStatus::Published),
    )
    .await
    .unwrap();
    let (c, h, _) = pop.get("/api/public/guides/stringing", &[]).await;
    assert_eq!(c, StatusCode::OK);
    let etag = h[header::ETAG].to_str().unwrap().to_owned();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    assert_eq!(h[header::CACHE_CONTROL], "public, max-age=60");
    let (c, h, b) = pop
        .get("/api/public/guides/stringing", &[("if-none-match", &etag)])
        .await;
    assert_eq!(c, StatusCode::NOT_MODIFIED);
    assert!(b.is_empty());
    assert_eq!(h[header::ETAG].to_str().unwrap(), etag);
    // Edited: the old ETag no longer matches.
    let mut g = guide("stringing", "Stringing, again", GuideStatus::Published);
    g.summary = "Changed.".into();
    guides::update(&pop.db, "stringing", &g).await.unwrap();
    let (c, h, _) = pop
        .get("/api/public/guides/stringing", &[("if-none-match", &etag)])
        .await;
    assert_eq!(c, StatusCode::OK);
    assert_ne!(h[header::ETAG].to_str().unwrap(), etag);
}

#[tokio::test]
async fn the_license_is_shown_and_indexing_is_off_unless_allowed() {
    let pop1 = pop(Guides::default()).await;
    guides::create(
        &pop1.db,
        &guide("stringing", "Stringing", GuideStatus::Published),
    )
    .await
    .unwrap();
    let (_, h, b) = pop1.get("/api/public/guides/stringing", &[]).await;
    assert_eq!(h["x-robots-tag"], "noindex");
    assert_eq!(
        serde_json::from_slice::<Value>(&b).unwrap()["license"],
        Value::Null
    );
    let (_, h, _) = pop1.get("/api/public/guides", &[]).await;
    assert_eq!(h["x-robots-tag"], "noindex");

    let pop2 = pop(Guides {
        license: "CC BY-SA 4.0".into(),
        indexable: true,
    })
    .await;
    guides::create(
        &pop2.db,
        &guide("stringing", "Stringing", GuideStatus::Published),
    )
    .await
    .unwrap();
    let (_, h, b) = pop2.get("/api/public/guides/stringing", &[]).await;
    assert!(h.get("x-robots-tag").is_none());
    assert_eq!(
        serde_json::from_slice::<Value>(&b).unwrap()["license"],
        "CC BY-SA 4.0"
    );
}

#[tokio::test]
async fn a_public_guide_cannot_be_changed_through_its_public_address() {
    // The second oracle for #7's check: what the table says, the server does.
    let pop = pop(Guides::default()).await;
    let g = guide("stringing", "Stringing", GuideStatus::Published);
    guides::create(&pop.db, &g).await.unwrap();
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        for uri in ["/api/public/guides", "/api/public/guides/stringing"] {
            let mut req = Request::builder()
                .method(method.clone())
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&guide("stringing", "Defaced", GuideStatus::Published))
                        .unwrap(),
                ))
                .unwrap();
            req.extensions_mut()
                .insert(ConnectInfo(STRANGER.parse::<SocketAddr>().unwrap()));
            let res = pop.app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {uri}"
            );
        }
    }
    assert_eq!(guides::get(&pop.db, "stringing").await.unwrap(), Some(g));
    assert_eq!(
        guides::check(
            &guides::get(&pop.db, "stringing").await.unwrap().unwrap(),
            &BTreeSet::new()
        ),
        []
    );
}
