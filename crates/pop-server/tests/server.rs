//! The server through its router, on a real SQLite database in a temporary
//! directory, with no network: each request goes straight into the router.

use std::{borrow::Cow, collections::HashMap, path::Path, sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    api::{ServerEvent, Status},
    assets::{Asset, Assets},
    db, router,
};
use tower::ServiceExt;

/// Assets held in memory, standing in for a built web app.
struct Fake(HashMap<&'static str, (&'static str, &'static str)>);

impl Assets for Fake {
    fn get(&self, path: &str) -> Option<Asset> {
        self.0.get(path).map(|(mime, body)| Asset {
            bytes: Cow::Borrowed(body.as_bytes()),
            mime: (*mime).to_owned(),
        })
    }
}

const INDEX: &str = "<!doctype html><title>POP</title><div id=app></div>";

fn built_app() -> Fake {
    Fake(HashMap::from([
        ("index.html", ("text/html", INDEX)),
        (
            "assets/app-1a2b3c.js",
            ("text/javascript", "console.log('pop')"),
        ),
        ("favicon.svg", ("image/svg+xml", "<svg/>")),
    ]))
}

struct Pop {
    _dir: tempfile::TempDir,
    state: AppState,
}

impl Pop {
    async fn new(assets: impl Assets + 'static) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("pop.db")).await.unwrap();
        Self {
            _dir: dir,
            state: AppState::new(pool, Arc::new(assets)),
        }
    }

    fn app(&self) -> Router {
        router(self.state.clone())
    }

    async fn get(&self, uri: &str) -> Response {
        self.app()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
}

async fn text(res: Response) -> String {
    String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
}

fn header_of(res: &Response, name: header::HeaderName) -> &str {
    res.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// The newest migration, read from the migration files themselves rather than
/// from anything the server computes, so this does not agree with the server
/// by construction.
fn newest_migration() -> i64 {
    std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            name.split('_').next()?.parse::<i64>().ok()
        })
        .max()
        .expect("at least one migration")
}

#[tokio::test]
async fn healthz_is_ok_on_a_fresh_database() {
    let pop = Pop::new(built_app()).await;
    let res = pop.get("/healthz").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(text(res).await, "ok");
}

#[tokio::test]
async fn healthz_fails_when_the_database_is_gone() {
    let pop = Pop::new(built_app()).await;
    pop.state.db.close().await;
    assert_eq!(
        pop.get("/healthz").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn status_reports_the_version_and_the_newest_migration() {
    let pop = Pop::new(built_app()).await;
    let res = pop.get("/api/status").await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["schema_version"], newest_migration());
}

#[tokio::test]
async fn an_unknown_api_path_is_a_json_404_never_the_app() {
    let pop = Pop::new(built_app()).await;
    for uri in ["/api/nope", "/api", "/api/status/extra"] {
        let res = pop.get(uri).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
        assert!(
            header_of(&res, header::CONTENT_TYPE).starts_with("application/json"),
            "{uri}"
        );
        let body = text(res).await;
        assert!(!body.contains("<!doctype"), "{uri} served the app: {body}");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            json["error"].as_str().unwrap().contains(uri),
            "{uri}: {json}"
        );
    }
}

#[tokio::test]
async fn a_screen_of_the_app_is_index_html_and_never_cached() {
    let pop = Pop::new(built_app()).await;
    for uri in ["/", "/kiosk", "/jobs/42", "/admin/printers/x1"] {
        let res = pop.get(uri).await;
        assert_eq!(res.status(), StatusCode::OK, "{uri}");
        assert_eq!(header_of(&res, header::CONTENT_TYPE), "text/html", "{uri}");
        assert_eq!(header_of(&res, header::CACHE_CONTROL), "no-cache", "{uri}");
        assert_eq!(text(res).await, INDEX, "{uri}");
    }
}

#[tokio::test]
async fn a_hashed_asset_is_served_with_its_type_and_cached_for_good() {
    let pop = Pop::new(built_app()).await;
    let res = pop.get("/assets/app-1a2b3c.js").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header_of(&res, header::CONTENT_TYPE), "text/javascript");
    assert_eq!(
        header_of(&res, header::CACHE_CONTROL),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(text(res).await, "console.log('pop')");
}

#[tokio::test]
async fn an_unhashed_file_is_served_but_not_cached_for_good() {
    let pop = Pop::new(built_app()).await;
    let res = pop.get("/favicon.svg").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header_of(&res, header::CACHE_CONTROL), "no-cache");
}

#[tokio::test]
async fn a_missing_file_is_a_404_not_the_app() {
    let pop = Pop::new(built_app()).await;
    for uri in ["/assets/app-gone.js", "/robots.txt", "/jobs/42/print.gcode"] {
        let res = pop.get(uri).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
        assert!(!text(res).await.contains("<!doctype"), "{uri}");
    }
}

#[tokio::test]
async fn without_a_built_app_screens_say_how_to_build_it() {
    let pop = Pop::new(Fake(HashMap::new())).await;
    let res = pop.get("/kiosk").await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(text(res).await.contains("tools/web/build.sh"));
    // The API does not depend on the app.
    assert_eq!(pop.get("/api/status").await.status(), StatusCode::OK);
}

// ---- live updates ------------------------------------------------------------

/// One `id:`/`data:` frame of an event stream.
#[derive(Debug, PartialEq)]
struct Frame {
    id: u64,
    data: serde_json::Value,
}

/// Reads frames off a live event stream, which never ends on its own.
struct Reader {
    body: Body,
    buf: String,
}

impl Reader {
    async fn open(pop: &Pop) -> Self {
        let res = pop.get("/api/events").await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header_of(&res, header::CONTENT_TYPE), "text/event-stream");
        Self {
            body: res.into_body(),
            buf: String::new(),
        }
    }

    /// The next frame with data, skipping keep-alive comments. Fails rather
    /// than hangs if none arrives within two seconds.
    async fn next(&mut self) -> Frame {
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..end + 2).collect();
                let field = |name: &str| {
                    block
                        .lines()
                        .find_map(|l| l.strip_prefix(name).map(|v| v.trim_start().to_owned()))
                };
                if let Some(data) = field("data:") {
                    return Frame {
                        id: field("id:")
                            .expect("every frame carries an id")
                            .parse()
                            .unwrap(),
                        data: serde_json::from_str(&data).unwrap(),
                    };
                }
                continue;
            }
            let frame = tokio::time::timeout(Duration::from_secs(2), self.body.frame())
                .await
                .expect("no event within two seconds")
                .expect("the stream ended")
                .unwrap();
            let bytes: Bytes = frame.into_data().unwrap_or_default();
            self.buf.push_str(std::str::from_utf8(&bytes).unwrap());
        }
    }
}

fn status_event(schema_version: i64) -> ServerEvent {
    ServerEvent::Status(Status {
        version: "test".into(),
        schema_version,
    })
}

#[tokio::test]
async fn a_screen_is_sent_a_snapshot_as_soon_as_it_connects() {
    let pop = Pop::new(built_app()).await;
    let mut r = Reader::open(&pop).await;
    let first = r.next().await;
    assert_eq!(first.data["type"], "status");
    assert_eq!(first.data["data"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(first.data["data"]["schema_version"], newest_migration());
}

#[tokio::test]
async fn a_published_event_reaches_a_connected_screen_in_order() {
    let pop = Pop::new(built_app()).await;
    let mut r = Reader::open(&pop).await;
    let snapshot = r.next().await;
    pop.state.events.publish(status_event(101));
    pop.state.events.publish(status_event(102));
    let a = r.next().await;
    let b = r.next().await;
    assert_eq!(a.data["data"]["schema_version"], 101);
    assert_eq!(b.data["data"]["schema_version"], 102);
    assert!(
        snapshot.id < a.id && a.id < b.id,
        "ids {} {} {}",
        snapshot.id,
        a.id,
        b.id
    );
}

#[tokio::test]
async fn every_connected_screen_receives_the_same_event() {
    let pop = Pop::new(built_app()).await;
    let mut kiosk = Reader::open(&pop).await;
    let mut phone = Reader::open(&pop).await;
    kiosk.next().await;
    phone.next().await;
    pop.state.events.publish(status_event(7));
    assert_eq!(kiosk.next().await, phone.next().await);
}

#[tokio::test]
async fn a_screen_that_falls_behind_gets_a_fresh_snapshot_not_a_gap() {
    let pop = Pop::new(built_app()).await;
    let mut r = Reader::open(&pop).await;
    r.next().await;
    // Far more than the backlog, while the screen reads nothing.
    let n = 2_000;
    for i in 0..n {
        pop.state.events.publish(status_event(i));
    }
    let after = r.next().await;
    // A snapshot is the server's real status, carrying the newest id. The
    // oldest surviving event would instead carry one of the test's made-up
    // schema versions and an older id.
    assert_eq!(
        after.data["data"]["version"],
        env!("CARGO_PKG_VERSION"),
        "{after:?}"
    );
    assert_eq!(
        after.data["data"]["schema_version"],
        newest_migration(),
        "{after:?}"
    );
    assert_eq!(after.id, n as u64, "{after:?}");
}

#[tokio::test]
async fn closing_ends_every_stream_so_the_server_can_shut_down() {
    let pop = Pop::new(built_app()).await;
    let mut kiosk = Reader::open(&pop).await;
    kiosk.next().await;
    pop.state.events.close();
    let end = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(f) = kiosk.body.frame().await {
            f.unwrap();
        }
    })
    .await;
    assert!(end.is_ok(), "an open stream outlived the close");
    // One opened after the close ends at once: the screen gets its snapshot
    // from the next server, when it reconnects.
    let mut late = Reader::open(&pop).await;
    let end = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(f) = late.body.frame().await {
            f.unwrap();
        }
    })
    .await;
    assert!(end.is_ok(), "a stream opened after the close stayed open");
}
