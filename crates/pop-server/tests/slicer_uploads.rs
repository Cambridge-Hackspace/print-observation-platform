//! The slicer's upload (#37): PrusaSlicer, SuperSlicer and OrcaSlicer add
//! the tower as an OctoPrint, and what they send is stored as a print that
//! waits for its owner.
//!
//! Each request in fixtures/slicer-uploads/ was sent by libcurl through the
//! same calls the slicer's own code makes, at its pinned version, and
//! recorded byte for byte (tools/slicer-captures/README.md). Here each is
//! replayed as it was recorded, its captured key swapped for a live one.
//!
//! What this does not prove: the bytes of each slicer's own build of libcurl
//! (they bundle their own; these came from libcurl 8.18.0), and Cura, whose
//! OctoPrint support is a third-party plugin that wants more of OctoPrint
//! than an upload (see the PR for #37).

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pop_server::{
    AppState,
    api::Role,
    assets::Embedded,
    audit::Actor,
    backup, config, db, router,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const REMOTE: &str = "203.0.113.9:5000";

fn capture(name: &str) -> Vec<u8> {
    let p = format!(
        "{}/tests/fixtures/slicer-uploads/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn gcode_fixture(name: &str) -> Vec<u8> {
    let p = format!(
        "{}/../pop-gcode/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

struct Pop {
    dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    grace_id: i64,
    grace_cookie: String,
    key: String,
}

async fn pop_with(limits: config::Prints) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let u = users::create(
        &db,
        NewAccount {
            username: "grace",
            name: "grace",
            role: Role::Member,
            password: "a long enough password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let t = users::start_session(&db, &u, false).await.unwrap();
    let state = AppState::new(db.clone(), Arc::new(Embedded))
        .with_files_dir(dir.path().join(backup::FILES_DIR))
        .with_prints(limits);
    let app = router(state);
    let grace_cookie = format!("pop_session={t}");
    let mut pop = Pop {
        dir,
        db,
        app,
        grace_id: u.id,
        grace_cookie,
        key: String::new(),
    };
    let r = pop
        .json(Method::POST, "/api/keys", json!({"name": "Orca"}))
        .await;
    assert_eq!(r.0, StatusCode::CREATED, "{}", r.1);
    pop.key = r.1["secret"].as_str().unwrap().to_owned();
    pop
}

async fn pop() -> Pop {
    pop_with(config::Prints::default()).await
}

/// A recorded request, parsed back into its parts.
struct Recorded {
    method: Method,
    uri: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn parse(raw: &[u8]) -> Recorded {
    let at = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a head");
    let head = std::str::from_utf8(&raw[..at]).unwrap();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap().split(' ');
    let method = first.next().unwrap().parse().unwrap();
    let uri = first.next().unwrap().to_owned();
    let headers = lines
        .map(|l| {
            let (k, v) = l.split_once(':').unwrap();
            (k.to_owned(), v.trim().to_owned())
        })
        .collect();
    Recorded {
        method,
        uri,
        headers,
        body: raw[at + 4..].to_vec(),
    }
}

struct Res {
    code: StatusCode,
    body: Value,
}

impl Pop {
    async fn json(&self, method: Method, uri: &str, body: Value) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, &self.grace_cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(REMOTE.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let b = res.into_body().collect().await.unwrap().to_bytes();
        (code, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    /// Replay `raw` as it was recorded, with `key` in place of the one
    /// captured (None: with no key at all), and `extra` headers added.
    async fn replay(&self, raw: &[u8], key: Option<&str>, extra: &[(&str, &str)]) -> Res {
        let r = parse(raw);
        let mut req = Request::builder().method(r.method).uri(&r.uri);
        let mut had_key = false;
        for (k, v) in &r.headers {
            if k.eq_ignore_ascii_case("x-api-key") {
                had_key = true;
                assert_eq!(v, "CAPTURED-KEY");
                if let Some(key) = key {
                    req = req.header(k.as_str(), key);
                }
            } else {
                req = req.header(k.as_str(), v.as_str());
            }
        }
        assert!(had_key, "the slicer sent its key");
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        let mut req = req.body(Body::from(r.body)).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(REMOTE.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let b = res.into_body().collect().await.unwrap().to_bytes();
        Res {
            code,
            body: serde_json::from_slice(&b).unwrap_or(Value::Null),
        }
    }

    /// Grace's prints, as she sees them.
    async fn prints(&self) -> Vec<Value> {
        let mut req = Request::get("/api/prints")
            .header(header::COOKIE, &self.grace_cookie)
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(REMOTE.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let b = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&b)
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    fn print_file(&self, id: &str) -> Vec<u8> {
        std::fs::read(self.dir.path().join("files/prints").join(id)).unwrap()
    }

    /// Everything left in uploads/: nothing, once an upload is done with.
    fn arriving(&self) -> Vec<PathBuf> {
        match std::fs::read_dir(self.dir.path().join("uploads")) {
            Ok(d) => d.map(|e| e.unwrap().path()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

// ---- the connection test -------------------------------------------------------------

#[tokio::test]
async fn each_slicers_connection_test_passes_as_it_checks() {
    let pop = pop().await;
    for slicer in ["prusaslicer", "superslicer", "orcaslicer"] {
        let r = pop
            .replay(
                &capture(&format!("{slicer}-version.http")),
                Some(&pop.key),
                &[],
            )
            .await;
        assert_eq!(r.code, StatusCode::OK, "{slicer}: {}", r.body);
        // What each one's OctoPrint::test requires, from its source: an
        // `api` field, and `text` starting "OctoPrint".
        assert!(r.body["api"].is_string(), "{slicer}: {}", r.body);
        assert!(
            r.body["text"].as_str().unwrap().starts_with("OctoPrint"),
            "{slicer}: {}",
            r.body
        );
    }
}

// ---- uploads ------------------------------------------------------------------------

#[tokio::test]
async fn each_slicers_upload_is_stored_as_its_owners_print() {
    let pop = pop().await;
    for (capture_name, filename, fixture, model) in [
        (
            "prusaslicer-upload.http",
            "plate_0.2mm_PLA_MK4S_2m.gcode",
            "prusaslicer-mk4s.gcode",
            "MK4S",
        ),
        (
            "superslicer-upload.http",
            "plate_0.2mm_PLA_ENDER3V2_7m.gcode",
            "superslicer-ender3v2.gcode",
            "ENDER3V2",
        ),
        (
            "orcaslicer-upload.http",
            "plate_PETG_6m14s.gcode",
            "orcaslicer-ender3v2.gcode",
            "Creality Ender-3 V2",
        ),
    ] {
        let r = pop
            .replay(&capture(capture_name), Some(&pop.key), &[])
            .await;
        assert_eq!(r.code, StatusCode::CREATED, "{capture_name}: {}", r.body);
        assert_eq!(r.body["done"], true, "{capture_name}");
        assert_eq!(r.body["files"]["local"]["name"], filename, "{capture_name}");
        let mine = pop.prints().await;
        let p = mine
            .iter()
            .find(|p| p["filename"] == filename)
            .unwrap_or_else(|| panic!("{capture_name}: {mine:?}"));
        assert_eq!(p["owner"]["id"], pop.grace_id);
        assert_eq!(p["plates"][0]["printer_model"], model, "{capture_name}");
        // What arrived is the file the slicer read, byte for byte.
        assert_eq!(
            pop.print_file(p["id"].as_str().unwrap()),
            gcode_fixture(fixture),
            "{capture_name}"
        );
    }
    assert_eq!(pop.prints().await.len(), 3);
    assert_eq!(pop.arriving(), Vec::<PathBuf>::new());
    // Each is recorded, as Grace's, her key speaking for her.
    let stored: Vec<(Option<String>, String)> = sqlx::query_as(
        "SELECT actor_name, summary FROM audit WHERE kind = 'print_stored' ORDER BY id",
    )
    .fetch_all(&pop.db)
    .await
    .unwrap();
    assert_eq!(
        stored,
        [
            "plate_0.2mm_PLA_MK4S_2m.gcode",
            "plate_0.2mm_PLA_ENDER3V2_7m.gcode",
            "plate_PETG_6m14s.gcode",
        ]
        .map(|f| (
            Some("grace".to_owned()),
            format!("grace stored the print {f:?}")
        ))
    );
}

#[tokio::test]
async fn upload_and_print_is_taken_as_upload() {
    let pop = pop().await;
    for c in [
        "prusaslicer-upload-and-print.http",
        "orcaslicer-upload-3mf-and-print.http",
    ] {
        let r = pop.replay(&capture(c), Some(&pop.key), &[]).await;
        assert_eq!(r.code, StatusCode::CREATED, "{c}: {}", r.body);
        // It waits for its owner at the tower; the slicer is told so.
        assert_eq!(r.body["effectivePrint"], false, "{c}");
        assert_eq!(r.body["effectiveSelect"], false, "{c}");
    }
    let mine = pop.prints().await;
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().all(|p| p["expired_at"].is_null()), "both wait");
    let archive = mine
        .iter()
        .find(|p| p["format"] == "gcode.3mf")
        .expect("OrcaSlicer's .gcode.3mf");
    assert_eq!(archive["filename"], "plate_PETG_6m14s.gcode.3mf");
    assert_eq!(archive["plates"][0]["has_thumbnail"], true);
    assert_eq!(
        pop.print_file(archive["id"].as_str().unwrap()),
        gcode_fixture("orcaslicer-ender3v2.gcode.3mf")
    );
}

// ---- refusals ------------------------------------------------------------------------

#[tokio::test]
async fn without_its_key_an_upload_stores_nothing() {
    let pop = pop().await;
    let raw = capture("prusaslicer-upload.http");
    let r = pop.replay(&raw, None, &[]).await;
    assert_eq!(r.code, StatusCode::UNAUTHORIZED, "{}", r.body);
    let r = pop.replay(&raw, Some("pop_not-a-key"), &[]).await;
    assert_eq!(r.code, StatusCode::UNAUTHORIZED, "{}", r.body);
    // A member's session does not stand in for a key.
    let cookie = pop.grace_cookie.clone();
    let r = pop
        .replay(&raw, None, &[(header::COOKIE.as_str(), cookie.as_str())])
        .await;
    assert_eq!(r.code, StatusCode::UNAUTHORIZED, "{}", r.body);
    assert_eq!(pop.prints().await, Vec::<Value>::new());
    assert_eq!(pop.arriving(), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn a_print_past_the_limit_is_refused_as_it_arrives_and_leaves_nothing() {
    // The captured file is 59,820 bytes; with a limit of 1 MB the same form
    // fits. Below it, the file is made larger than the limit.
    let limits = config::Prints {
        max_file_mb: 1,
        member_mb: 1,
        ..config::Prints::default()
    };
    let pop = pop_with(limits).await;
    let raw = capture("prusaslicer-upload.http");
    let r = pop.replay(&raw, Some(&pop.key), &[]).await;
    assert_eq!(r.code, StatusCode::CREATED, "{}", r.body);

    // The same form with the file 1 MiB and a byte: the G-code repeated,
    // the multipart's boundary and length kept right.
    let rec = parse(&raw);
    let ct = rec
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .unwrap()
        .1
        .clone();
    let boundary = ct.split("boundary=").nth(1).unwrap();
    let mut big = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"big.gcode\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    big.extend(std::iter::repeat_n(b'\n', (1 << 20) + 1));
    big.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut req = Request::post("/api/files/local")
        .header("X-Api-Key", &pop.key)
        .header(header::CONTENT_TYPE, ct.as_str())
        .body(Body::from(big))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(REMOTE.parse::<SocketAddr>().unwrap()));
    let res = pop.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(pop.prints().await.len(), 1, "only the one that fit");
    assert_eq!(pop.arriving(), Vec::<PathBuf>::new(), "the part is deleted");
}

#[tokio::test]
async fn an_upload_cut_short_leaves_nothing() {
    let pop = pop().await;
    let mut raw = capture("superslicer-upload.http");
    // Half the body, its Content-Length still saying all of it.
    raw.truncate(raw.len() / 2);
    let r = pop.replay(&raw, Some(&pop.key), &[]).await;
    assert_eq!(r.code, StatusCode::BAD_REQUEST, "{}", r.body);
    assert_eq!(pop.prints().await, Vec::<Value>::new());
    assert_eq!(pop.arriving(), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn a_full_member_is_told_why_in_the_slicer() {
    let limits = config::Prints {
        max_waiting: 1,
        ..config::Prints::default()
    };
    let pop = pop_with(limits).await;
    let raw = capture("orcaslicer-upload.http");
    assert_eq!(
        pop.replay(&raw, Some(&pop.key), &[]).await.code,
        StatusCode::CREATED
    );
    let r = pop.replay(&raw, Some(&pop.key), &[]).await;
    assert_eq!(r.code, StatusCode::CONFLICT);
    // The slicers show the body's text in their error.
    assert!(
        r.body["error"]
            .as_str()
            .unwrap()
            .contains("at most 1 prints"),
        "{}",
        r.body
    );
    assert_eq!(pop.arriving(), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn what_is_not_a_slicers_form_is_refused() {
    let pop = pop().await;
    for (ct, body, why) in [
        ("application/json", "{}".to_owned(), "not a form"),
        (
            "multipart/form-data; boundary=x",
            "--x\r\nContent-Disposition: form-data; name=\"print\"\r\n\r\ntrue\r\n--x--\r\n".to_owned(),
            "no file",
        ),
        (
            "multipart/form-data; boundary=x",
            "--x\r\nContent-Disposition: form-data; name=\"file\"; filename=\"model.stl\"\r\n\r\nsolid x\r\n--x--\r\n"
                .to_owned(),
            "not a print",
        ),
    ] {
        let mut req = Request::post("/api/files/local")
            .header("X-Api-Key", &pop.key)
            .header(header::CONTENT_TYPE, ct)
            .body(Body::from(body))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(REMOTE.parse::<SocketAddr>().unwrap()));
        let res = pop.app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{why}");
    }
    assert_eq!(pop.prints().await, Vec::<Value>::new());
    assert_eq!(pop.arriving(), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn only_a_key_route_takes_a_form() {
    // The JSON rule still holds for every route a cookie reaches: the
    // exemption is for routes a key alone opens.
    for r in pop_server::routes() {
        let writes = matches!(r.method, Method::POST | Method::PUT | Method::PATCH);
        if writes && r.need == pop_server::access::Need::Key {
            assert_eq!((r.method.as_str(), r.path), ("POST", "/api/files/local"));
        }
    }
    let pop = pop().await;
    let mut req = Request::post("/api/keys")
        .header(header::COOKIE, &pop.grace_cookie)
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=x")
        .body(Body::from(
            "--x\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nx\r\n--x--\r\n",
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(REMOTE.parse::<SocketAddr>().unwrap()));
    let res = pop.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(n, 1, "no key was made by a form");
}

/// A form of `parts` (name, filename, bytes) with boundary "x".
fn form(parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut b = Vec::new();
    for (name, filename, bytes) in parts {
        b.extend_from_slice(b"--x\r\nContent-Disposition: form-data; name=\"");
        b.extend_from_slice(name.as_bytes());
        b.push(b'"');
        if let Some(f) = filename {
            b.extend_from_slice(format!("; filename=\"{f}\"").as_bytes());
        }
        b.extend_from_slice(b"\r\n\r\n");
        b.extend_from_slice(bytes);
        b.extend_from_slice(b"\r\n");
    }
    b.extend_from_slice(b"--x--\r\n");
    b
}

impl Pop {
    async fn post_form(&self, body: Vec<u8>) -> StatusCode {
        let mut req = Request::post("/api/files/local")
            .header("X-Api-Key", &self.key)
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=x")
            .body(Body::from(body))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(REMOTE.parse::<SocketAddr>().unwrap()));
        self.app.clone().oneshot(req).await.unwrap().status()
    }
}

#[tokio::test]
async fn a_print_larger_than_a_request_usually_may_be_is_taken() {
    // Past the server's default 2 MB, within [prints].max_file_mb.
    let pop = pop().await;
    let big = vec![b'\n'; 3 << 20];
    let code = pop
        .post_form(form(&[("file", Some("big.gcode"), &big)]))
        .await;
    assert_eq!(code, StatusCode::CREATED);
    assert_eq!(pop.prints().await.len(), 1);
}

#[tokio::test]
async fn a_form_is_bounded_in_its_fields() {
    let pop = pop().await;
    let print = b"; printer_model = MK4S\n";
    let long = vec![b'a'; 1025];
    assert_eq!(
        pop.post_form(form(&[
            ("path", None, &long),
            ("file", Some("a.gcode"), print)
        ]))
        .await,
        StatusCode::BAD_REQUEST,
        "a field past 1 KiB"
    );
    let ok = vec![b'a'; 1024];
    assert_eq!(
        pop.post_form(form(&[
            ("path", None, &ok),
            ("file", Some("a.gcode"), print)
        ]))
        .await,
        StatusCode::CREATED,
        "a field of 1 KiB"
    );
    let mut many: Vec<(&str, Option<&str>, &[u8])> = vec![("print", None, b"false"); 31];
    many.push(("file", Some("b.gcode"), print));
    assert_eq!(
        pop.post_form(form(&many)).await,
        StatusCode::CREATED,
        "32 fields"
    );
    many.insert(0, ("print", None, b"false"));
    assert_eq!(
        pop.post_form(form(&many)).await,
        StatusCode::BAD_REQUEST,
        "33 fields"
    );
    assert_eq!(
        pop.post_form(form(&[
            ("file", Some("c.gcode"), print),
            ("file", Some("d.gcode"), print)
        ]))
        .await,
        StatusCode::BAD_REQUEST,
        "two files"
    );
    assert_eq!(pop.prints().await.len(), 2);
    assert_eq!(pop.arriving(), Vec::<PathBuf>::new());
}
