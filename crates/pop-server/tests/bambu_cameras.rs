//! Bambu printers' own cameras (#33): an X1's RTSPS camera played by go2rtc
//! through POP's pinned relay, and an A1's JPEG pictures read by POP itself.
//! Against a fake go2rtc and fake Bambu cameras.
//!
//! What this does not prove: that a real X1 accepts RTSP through the relay,
//! or that a real A1 sends what the sources describe. Neither has been
//! watched by POP yet (docs/printers/bambu.md).

use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};

use axum::Router;
use pop_integrations::{
    fakes::go2rtc::{self as fake_go2rtc, FakeGo2rtc},
    go2rtc::Go2rtc,
    printers::{Camera as PrinterCamera, Connection, PrinterConfig},
};
use pop_printers::{
    bambu::{MemoryPins, PinStore},
    bambu_camera::BambuCamera,
    fakes::bambu_camera::{FakeBambuCamera, JPEG},
};
use pop_server::{
    AppState,
    api::{AdminCamera, CameraHealth, Role},
    assets::Embedded,
    audit::Actor,
    cameras::{self, Bambu, Camera, Source, source_of, stream_name},
    db, router,
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

const GO2RTC: &str =
    include_str!("../../pop-integrations/tests/recordings/go2rtc-1.9.15/recordings.json");
const SERIAL: &str = "01P00A123456789";
const CODE: &str = "12345678";

struct Pop {
    _dir: tempfile::TempDir,
    go2rtc: FakeGo2rtc,
    health: watch::Receiver<Vec<AdminCamera>>,
    pins: Arc<MemoryPins>,
    addr: String,
    admin: String,
}

/// POP with these cameras, served on a real port, and Ada signed in.
async fn pop(wanted: cameras::Wanted) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    users::create(
        &db,
        NewAccount {
            username: "ada",
            name: "Ada",
            role: Role::Admin,
            password: "ada's password",
        },
        &Actor::command_line(),
    )
    .await
    .unwrap();
    let go2rtc = FakeGo2rtc::start(GO2RTC).await;
    let client = Go2rtc::new(
        go2rtc.url(),
        Some((fake_go2rtc::USER.into(), fake_go2rtc::PASSWORD.into())),
    )
    .unwrap();
    let pins = Arc::new(MemoryPins::default());
    let (_tx, rx) = watch::channel(wanted);
    // The sender lives as long as the test: leaked, so the keeper keeps going.
    std::mem::forget(_tx);
    let (kept, _) = cameras::spawn(
        client.clone(),
        rx,
        cameras::Options {
            every: Duration::from_millis(100),
        },
        pins.clone(),
    );
    let health = kept.health.clone();
    let app: Router = router(AppState::new(db, Arc::new(Embedded)).with_cameras(kept, client));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    let res = reqwest::Client::new()
        .post(format!("http://{addr}/api/session"))
        .json(&json!({"username": "ada", "password": "ada's password"}))
        .send()
        .await
        .unwrap();
    let admin = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Pop {
        _dir: dir,
        go2rtc,
        health,
        pins,
        addr,
        admin,
    }
}

fn bambu(fake: &FakeBambuCamera, code: &str) -> Bambu {
    Bambu {
        host: fake.host(),
        port: fake.port(),
        serial: SERIAL.into(),
        access_code: code.into(),
    }
}

fn one(id: &str, name: &str, source: Source) -> cameras::Wanted {
    BTreeMap::from([(
        id.to_owned(),
        Camera {
            name: name.into(),
            source,
        },
    )])
}

impl Pop {
    async fn health_until(&self, what: &str, ok: impl Fn(&AdminCamera) -> bool) -> AdminCamera {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(c) = self.health.borrow().first().filter(|c| ok(c)) {
                return c.clone();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "never {what}: {:#?}",
                *self.health.borrow()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn admin_cameras(&self) -> String {
        reqwest::Client::new()
            .get(format!("http://{}/api/admin/cameras", self.addr))
            .header("Cookie", &self.admin)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }
}

// ---- which camera a printer has ---------------------------------------------------

fn config(camera: Option<PrinterCamera>, connection: Connection) -> PrinterConfig {
    PrinterConfig {
        bam_id: "p1".into(),
        name: "A printer".into(),
        connection,
        camera,
        model: None,
        materials: vec![],
        training: None,
    }
}

#[test]
fn a_bambu_printers_own_camera_is_on_the_port_its_kind_says() {
    let x1 = Connection::Bambu {
        host: "192.168.1.50".into(),
        serial: SERIAL.into(),
    };
    let own = |port| Bambu {
        host: "192.168.1.50".into(),
        port,
        serial: SERIAL.into(),
        access_code: CODE.into(),
    };
    assert_eq!(
        source_of(
            &config(Some(PrinterCamera::BambuRtsps), x1.clone()),
            Some(CODE),
            None
        ),
        Some(Source::BambuRtsps(own(322)))
    );
    assert_eq!(
        source_of(
            &config(Some(PrinterCamera::BambuJpeg), x1.clone()),
            Some(CODE),
            None
        ),
        Some(Source::BambuJpeg(own(6000)))
    );
    // No access code: nothing to sign in with, so no camera yet.
    assert_eq!(
        source_of(
            &config(Some(PrinterCamera::BambuRtsps), x1.clone()),
            None,
            None
        ),
        None
    );
    let url = "rtsp://10.0.0.40/stream";
    assert_eq!(
        source_of(
            &config(Some(PrinterCamera::Address { url: url.into() }), x1),
            Some(CODE),
            None
        ),
        Some(Source::Address(url.into(), None))
    );
}

// ---- an X1: RTSPS, through the relay ------------------------------------------------

async fn ask(port: u16) -> String {
    let mut local = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    local
        .write_all(b"OPTIONS rtsp://127.0.0.1/streaming/live/1 RTSP/1.0\r\nCSeq: 1\r\n\r\n")
        .await
        .unwrap();
    let mut got = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(5), local.read(&mut got))
        .await
        .map_or(0, |r| r.unwrap_or(0));
    String::from_utf8_lossy(&got[..n]).into_owned()
}

/// go2rtc is given the relay's loopback port, signed in with the access
/// code; what it sends there reaches the printer, over TLS with the pin.
#[tokio::test]
async fn an_x1s_camera_is_played_by_go2rtc_through_the_pinned_relay() {
    let x1 = FakeBambuCamera::start_rtsps(SERIAL).await;
    let pop = pop(one("x1", "X1 Carbon", Source::BambuRtsps(bambu(&x1, CODE)))).await;
    let name = stream_name("x1");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let source = loop {
        if let Some(s) = pop.go2rtc.streams().get(&name) {
            break s[0].clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "go2rtc was never given the X1"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let rest = source
        .strip_prefix(&format!("rtsp://bblp:{CODE}@127.0.0.1:"))
        .unwrap_or_else(|| panic!("{source}"));
    let (port, path) = rest.split_once('/').unwrap();
    assert_eq!(path, "streaming/live/1");
    // Never the printer itself: go2rtc would not check its certificate.
    assert!(!source.contains(&x1.host()) || x1.host() == "127.0.0.1");
    assert_ne!(port.parse::<u16>().unwrap(), x1.port());
    let reply = ask(port.parse().unwrap()).await;
    assert!(reply.starts_with("RTSP/1.0 200 OK"), "{reply:?}");
    assert_eq!(x1.rtsp_requests().len(), 1);
    assert_eq!(
        pop.pins.pinned(&BambuCamera::pin_key(SERIAL)),
        Some(x1.fingerprint())
    );
    // Admins see the stream, and never the access code.
    let listed = pop.admin_cameras().await;
    assert!(listed.contains(&name), "{listed}");
    assert!(!listed.contains(CODE), "{listed}");
}

#[tokio::test]
async fn an_x1_with_another_certificate_gets_nothing_and_admins_are_told_why() {
    let x1 = FakeBambuCamera::start_rtsps(SERIAL).await;
    let pop = pop(one("x1", "X1 Carbon", Source::BambuRtsps(bambu(&x1, CODE)))).await;
    let name = stream_name("x1");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !pop.go2rtc.streams().contains_key(&name) {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let source = pop.go2rtc.streams()[&name][0].clone();
    let port: u16 = source
        .rsplit_once(':')
        .unwrap()
        .1
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(ask(port).await.starts_with("RTSP/1.0 200"));
    x1.new_certificate(SERIAL);
    assert_eq!(ask(port).await, "", "nothing came back");
    assert_eq!(x1.rtsp_requests().len(), 1, "and nothing reached it");
    let c = pop
        .health_until("said the certificate", |c| {
            c.detail
                .as_deref()
                .is_some_and(|d| d.contains("certificate"))
        })
        .await;
    assert_ne!(c.health, CameraHealth::Live);
    assert!(c.detail.unwrap().contains(&x1.fingerprint()));
}

// ---- an A1: JPEG pictures, read by POP ---------------------------------------------

#[tokio::test]
async fn an_a1s_pictures_are_read_once_however_many_watch() {
    let a1 = FakeBambuCamera::start(SERIAL, CODE, Duration::from_millis(30)).await;
    let pop = pop(one("a1", "A1 mini", Source::BambuJpeg(bambu(&a1, CODE)))).await;
    let c = pop
        .health_until("read a picture", |c| c.health == CameraHealth::Live)
        .await;
    assert_eq!(c.stream, None, "go2rtc is not involved");
    assert!(pop.go2rtc.streams().is_empty());

    let url = format!("http://{}/api/printers/a1/camera/mjpeg", pop.addr);
    let http = reqwest::Client::new();
    let mut viewers = Vec::new();
    for _ in 0..3 {
        let mut res = http
            .get(&url)
            .header("Cookie", &pop.admin)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(
            res.headers()["content-type"],
            "multipart/x-mixed-replace; boundary=frame"
        );
        let mut got = Vec::new();
        while !got.windows(JPEG.len()).any(|w| w == JPEG) {
            got.extend_from_slice(&res.chunk().await.unwrap().expect("pictures"));
        }
        assert!(got.starts_with(b"--frame\r\nContent-Type: image/jpeg\r\n"));
        viewers.push(res);
    }
    // A look's own brief connection may still be closing at the fake's end;
    // then there is one, and while they watch POP makes no other.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while a1.watching() != 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} connections",
            a1.watching()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let signed_in = a1.signed_in();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        a1.signed_in(),
        signed_in,
        "no new connection while three watch, through five looks"
    );
    assert_eq!(
        a1.watching(),
        1,
        "one connection to the printer for three viewers"
    );
    // MSE cannot carry JPEG pictures: said so.
    let (mut ws, _) = tokio_tungstenite::connect_async({
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut r = format!("ws://{}/api/printers/a1/camera/mse", pop.addr)
            .into_client_request()
            .unwrap();
        r.headers_mut().insert("Cookie", pop.admin.parse().unwrap());
        r
    })
    .await
    .unwrap();
    use futures_util::{SinkExt, StreamExt};
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({"type": "mse", "value": "avc1.640029"})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let said = ws.next().await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(said.to_text().unwrap()).unwrap(),
        json!({"type": "error", "value": "unsupported"})
    );
    // Everyone leaves: so does POP, once no look holds the camera either.
    drop(viewers);
    drop(ws);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while a1.watching() != 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "POP kept watching an empty room"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_a1_that_refuses_the_access_code_reads_no_picture_and_says_why() {
    let a1 = FakeBambuCamera::start(SERIAL, CODE, Duration::from_millis(30)).await;
    let pop = pop(one(
        "a1",
        "A1 mini",
        Source::BambuJpeg(bambu(&a1, "87654321")),
    ))
    .await;
    let c = pop.health_until("said why", |c| c.detail.is_some()).await;
    assert_eq!(c.health, CameraHealth::NoPicture);
    assert!(c.detail.unwrap().contains("access code"));
    let listed = pop.admin_cameras().await;
    assert!(!listed.contains("87654321"), "{listed}");
}

/// An access code is put in go2rtc's address as a URL's user part: a
/// character that means something there is escaped.
#[tokio::test]
async fn an_access_code_is_escaped_in_go2rtcs_address() {
    let x1 = FakeBambuCamera::start_rtsps(SERIAL).await;
    let pop = pop(one(
        "x1",
        "X1 Carbon",
        Source::BambuRtsps(bambu(&x1, "a@b/c:d")),
    ))
    .await;
    let name = stream_name("x1");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !pop.go2rtc.streams().contains_key(&name) {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let source = &pop.go2rtc.streams()[&name][0];
    assert!(
        source.starts_with("rtsp://bblp:a%40b%2Fc%3Ad@127.0.0.1:"),
        "{source}"
    );
}
