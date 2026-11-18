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
    db,
    health::{CheckFuture, HealthCheck},
    router,
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

/// An integration's health check that answers as told.
struct Integration(&'static str, Result<(), &'static str>);

impl HealthCheck for Integration {
    fn name(&self) -> &'static str {
        self.0
    }
    fn check(&self) -> CheckFuture<'_> {
        let r = self.1.map_err(str::to_owned);
        Box::pin(async move { r })
    }
}

async fn health(pop: &Pop) -> (StatusCode, serde_json::Value) {
    let res = pop.get("/healthz").await;
    let code = res.status();
    (code, serde_json::from_str(&text(res).await).unwrap())
}

#[tokio::test]
async fn healthz_reports_the_database_ok_on_a_fresh_database() {
    let pop = Pop::new(built_app()).await;
    let (code, body) = health(&pop).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["checks"]["database"], serde_json::json!({"ok": true}));
}

#[tokio::test]
async fn healthz_fails_naming_the_database_when_it_is_gone() {
    let pop = Pop::new(built_app()).await;
    pop.state.db.close().await;
    let (code, body) = health(&pop).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["ok"], false);
    assert_eq!(body["checks"]["database"]["ok"], false);
    // A sentence, not the driver's error, which can carry paths: this route is
    // reachable from outside the space.
    assert_eq!(
        body["checks"]["database"]["error"],
        "the database does not answer"
    );
}

#[tokio::test]
async fn healthz_names_each_integration_and_fails_on_any() {
    let mut pop = Pop::new(built_app()).await;
    pop.state = pop
        .state
        .clone()
        .with_check(Integration("bam", Ok(())))
        .with_check(Integration("go2rtc", Err("go2rtc does not answer")));
    let (code, body) = health(&pop).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["ok"], false);
    assert_eq!(body["checks"]["database"]["ok"], true);
    assert_eq!(body["checks"]["bam"], serde_json::json!({"ok": true}));
    assert_eq!(
        body["checks"]["go2rtc"],
        serde_json::json!({"ok": false, "error": "go2rtc does not answer"})
    );
}

#[tokio::test]
async fn healthz_is_ok_when_every_integration_is() {
    let mut pop = Pop::new(built_app()).await;
    pop.state = pop.state.clone().with_check(Integration("bam", Ok(())));
    let (code, body) = health(&pop).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn status_reports_the_version_and_the_newest_migration() {
    let pop = Pop::new(built_app()).await;
    let res = pop.get("/api/status").await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["schema_version"], newest_migration());
    assert_eq!(body["public_url"], serde_json::Value::Null, "none set");
}

#[tokio::test]
async fn status_says_where_phones_reach_pop_when_it_is_set() {
    let mut pop = Pop::new(built_app()).await;
    pop.state = pop.state.clone().with_public_url("https://pop.example.org");
    let body: serde_json::Value =
        serde_json::from_str(&text(pop.get("/api/status").await).await).unwrap();
    assert_eq!(body["public_url"], "https://pop.example.org");
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
async fn no_screen_is_indexed_but_the_public_guides_when_allowed() {
    let pop = Pop::new(built_app()).await;
    for uri in ["/", "/kiosk", "/guides", "/guides/stringing", "/admin"] {
        let res = pop.get(uri).await;
        assert_eq!(res.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            header_of(&res, "x-robots-tag".parse().unwrap()),
            "noindex",
            "{uri}"
        );
    }
    let mut pop = Pop::new(built_app()).await;
    pop.state = pop.state.clone().with_guides(pop_server::config::Guides {
        license: String::new(),
        indexable: true,
    });
    for uri in ["/guides", "/guides/stringing"] {
        assert!(
            pop.get(uri).await.headers().get("x-robots-tag").is_none(),
            "{uri}"
        );
    }
    for uri in ["/kiosk", "/", "/guidesx"] {
        let res = pop.get(uri).await;
        assert_eq!(
            header_of(&res, "x-robots-tag".parse().unwrap()),
            "noindex",
            "{uri}"
        );
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
#[derive(Debug, Clone, PartialEq)]
struct Frame {
    id: String,
    /// The id's number, after its epoch.
    n: u64,
    data: serde_json::Value,
}

/// How a reconnecting screen says what it saw last.
#[derive(Clone, Copy)]
enum Via {
    /// `Last-Event-ID`, as a browser's own reconnect sends it.
    Header,
    /// `?last_event_id=`, as the web app sends it on a connection it opens
    /// itself.
    Query,
}

/// Reads frames off a live event stream, which never ends on its own.
struct Reader {
    body: Body,
    buf: String,
}

impl Reader {
    async fn open(pop: &Pop) -> Self {
        Self::request(pop, Request::get("/api/events")).await
    }

    async fn resume(pop: &Pop, last: &str, via: Via) -> Self {
        let req = match via {
            Via::Header => Request::get("/api/events").header("last-event-id", last),
            Via::Query => Request::get(format!(
                "/api/events?last_event_id={}",
                last.replace('.', "%2E")
            )),
        };
        Self::request(pop, req).await
    }

    async fn request(pop: &Pop, req: axum::http::request::Builder) -> Self {
        let res = pop
            .app()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
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
        self.try_next(Duration::from_secs(2))
            .await
            .expect("no event within two seconds")
    }

    /// The next frame, or None if none arrives within `wait`.
    async fn try_next(&mut self, wait: Duration) -> Option<Frame> {
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..end + 2).collect();
                let field = |name: &str| {
                    block
                        .lines()
                        .find_map(|l| l.strip_prefix(name).map(|v| v.trim_start().to_owned()))
                };
                if let Some(data) = field("data:") {
                    let id = field("id:").expect("every frame carries an id");
                    let n = id
                        .rsplit_once('.')
                        .expect("an id is epoch.n")
                        .1
                        .parse()
                        .unwrap();
                    return Some(Frame {
                        id,
                        n,
                        data: serde_json::from_str(&data).unwrap(),
                    });
                }
                continue;
            }
            let frame = tokio::time::timeout(wait, self.body.frame()).await.ok()?;
            let bytes: Bytes = frame
                .expect("the stream ended")
                .unwrap()
                .into_data()
                .unwrap_or_default();
            self.buf.push_str(std::str::from_utf8(&bytes).unwrap());
        }
    }
}

/// What a screen shows, built from the stream the way the web app builds it:
/// a snapshot replaces everything, a status event replaces the status.
#[derive(Debug, Default, PartialEq)]
struct Screen {
    status: Option<serde_json::Value>,
    last: Option<String>,
    seen: Vec<u64>,
}

impl Screen {
    fn apply(&mut self, f: &Frame) {
        match f.data["type"].as_str() {
            Some("snapshot") => self.status = Some(f.data["data"]["status"].clone()),
            Some("status") => self.status = Some(f.data["data"].clone()),
            other => panic!("an event the screen does not know: {other:?}"),
        }
        self.last = Some(f.id.clone());
        self.seen.push(f.n);
    }
}

fn status_event(schema_version: i64) -> ServerEvent {
    ServerEvent::Status(Status {
        version: "test".into(),
        schema_version,
        public_url: None,
    })
}

#[tokio::test]
async fn a_screen_is_sent_a_snapshot_as_soon_as_it_connects() {
    let pop = Pop::new(built_app()).await;
    let mut r = Reader::open(&pop).await;
    let first = r.next().await;
    assert_eq!(first.data["type"], "snapshot");
    let status = &first.data["data"]["status"];
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["schema_version"], newest_migration());
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
        snapshot.n < a.n && a.n < b.n,
        "{} {} {}",
        snapshot.n,
        a.n,
        b.n
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
async fn a_screen_that_falls_behind_gets_a_fresh_snapshot_and_nothing_older_after_it() {
    let pop = Pop::new(built_app()).await;
    let mut r = Reader::open(&pop).await;
    r.next().await;
    // Far more than the backlog, while the screen reads nothing.
    let n = 2_000;
    for i in 0..n {
        pop.state.events.publish(status_event(i));
    }
    let after = r.next().await;
    assert_eq!(after.data["type"], "snapshot", "{after:?}");
    assert_eq!(after.n, n as u64, "it carries the newest id: {after:?}");
    // Events still queued for it are older than the snapshot, and already in
    // it: applying one would wind the screen backwards.
    pop.state.events.publish(status_event(9_999));
    let next = r.next().await;
    assert_eq!(
        next.n,
        n as u64 + 1,
        "the next frame is the new event, not a stale one"
    );
    assert_eq!(next.data["data"]["schema_version"], 9_999);
}

/// The issue's "done when": a screen whose connection drops mid-stream ends
/// in the same state as one that never dropped, having been sent every event
/// once, and only the ones it missed.
async fn drop_and_resume(via: Via) {
    let pop = Pop::new(built_app()).await;
    let (mut steady, mut flaky) = (Screen::default(), Screen::default());
    let mut a = Reader::open(&pop).await;
    let mut b = Reader::open(&pop).await;
    steady.apply(&a.next().await);
    flaky.apply(&b.next().await);
    for i in 0..10 {
        pop.state.events.publish(status_event(i));
    }
    for _ in 0..5 {
        flaky.apply(&b.next().await);
    }
    drop(b); // the connection drops, five events unread
    for i in 10..20 {
        pop.state.events.publish(status_event(i));
    }
    let mut b = Reader::resume(&pop, flaky.last.as_deref().unwrap(), via).await;
    for i in 20..25 {
        pop.state.events.publish(status_event(i));
    }
    for _ in 0..25 {
        steady.apply(&a.next().await);
    }
    for _ in 0..20 {
        flaky.apply(&b.next().await);
    }
    assert_eq!(flaky.status, steady.status);
    // Not just the same end: the same events, each once, with no snapshot in
    // place of the ones it missed.
    assert_eq!(flaky.seen, steady.seen);
    assert!(
        b.try_next(Duration::from_millis(300)).await.is_none(),
        "nothing extra"
    );
}

#[tokio::test]
async fn a_screen_that_drops_and_resumes_by_header_ends_where_a_steady_one_does() {
    drop_and_resume(Via::Header).await;
}

#[tokio::test]
async fn a_screen_that_drops_and_resumes_by_query_ends_where_a_steady_one_does() {
    drop_and_resume(Via::Query).await;
}

#[tokio::test]
async fn resuming_while_events_keep_coming_loses_and_repeats_none() {
    let pop = Pop::new(built_app()).await;
    let mut a = Reader::open(&pop).await;
    let first = a.next().await;
    let publisher = {
        let events = pop.state.events.clone();
        tokio::spawn(async move {
            for i in 0..200 {
                events.publish(status_event(i));
                tokio::task::yield_now().await;
            }
        })
    };
    // Reconnect over and over while the publisher runs, each time from the
    // last frame read.
    let mut seen = vec![first.n];
    let mut last = first.id.clone();
    while seen.last() != Some(&200) {
        let mut b = Reader::resume(&pop, &last, Via::Header).await;
        for _ in 0..3 {
            match b.try_next(Duration::from_millis(500)).await {
                Some(f) => {
                    assert_ne!(
                        f.data["type"], "snapshot",
                        "a resumable screen was sent a snapshot"
                    );
                    seen.push(f.n);
                    last = f.id;
                }
                None => break,
            }
        }
    }
    publisher.await.unwrap();
    assert_eq!(
        seen,
        (0..=200).collect::<Vec<u64>>(),
        "every event once, in order"
    );
    let _ = a;
}

/// A screen that cannot resume is sent a snapshot, and ends right all the same.
async fn resume_falls_back_to_a_snapshot(last: impl FnOnce(&str, u64) -> String) {
    let pop = Pop::new(built_app()).await;
    let mut a = Reader::open(&pop).await;
    let first = a.next().await;
    let epoch = first.id.rsplit_once('.').unwrap().0.to_owned();
    pop.state.events.publish(status_event(1));
    let mut b = Reader::resume(&pop, &last(&epoch, 1), Via::Header).await;
    let f = b.next().await;
    assert_eq!(f.data["type"], "snapshot", "{f:?}");
    assert_eq!(f.n, 1, "the snapshot carries the newest id");
}

#[tokio::test]
async fn an_id_from_before_a_restart_gets_a_snapshot() {
    resume_falls_back_to_a_snapshot(|_, n| format!("an-older-process.{n}")).await;
}

#[tokio::test]
async fn an_id_from_the_future_gets_a_snapshot() {
    resume_falls_back_to_a_snapshot(|epoch, _| format!("{epoch}.999")).await;
}

#[tokio::test]
async fn a_malformed_id_gets_a_snapshot() {
    resume_falls_back_to_a_snapshot(|_, _| "not an id".to_owned()).await;
}

#[tokio::test]
async fn an_id_too_old_to_replay_gets_a_snapshot() {
    let pop = Pop::new(built_app()).await;
    let mut a = Reader::open(&pop).await;
    let first = a.next().await;
    let total = pop_server::events::REPLAY as i64 + 50;
    for i in 0..total {
        pop.state.events.publish(status_event(i));
    }
    let mut b = Reader::resume(&pop, &first.id, Via::Header).await;
    let f = b.next().await;
    assert_eq!(f.data["type"], "snapshot", "{f:?}");
    assert_eq!(f.n, total as u64);
}

#[tokio::test]
async fn an_id_just_inside_the_buffer_replays() {
    let pop = Pop::new(built_app()).await;
    let mut a = Reader::open(&pop).await;
    let first = a.next().await;
    let replay = pop_server::events::REPLAY as i64;
    for i in 0..replay {
        pop.state.events.publish(status_event(i));
    }
    // It missed exactly REPLAY events, all still held.
    let mut b = Reader::resume(&pop, &first.id, Via::Header).await;
    let f = b.next().await;
    assert_eq!(f.data["type"], "status", "{f:?}");
    assert_eq!(f.n, 1);
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
