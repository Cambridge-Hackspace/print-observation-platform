//! POP's side of automatic upgrades (#148): it says when the tower is quiet,
//! to this machine only, and records what the updater did. The updater
//! itself is tested in crates/pop-updater.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pop_printers::{Job, PrinterState, PrinterStatus};
use pop_server::{
    AppState,
    api::{Role, TowerPrinter},
    assets::Embedded,
    audit::Actor,
    db, router,
    updater::{EVENTS, busy, ingest},
    users::{self, NewAccount},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::watch;
use tower::ServiceExt;

const LOCAL: &str = "127.0.0.1:4000";
const REMOTE: &str = "203.0.113.9:4000";

fn printer(name: &str, status: PrinterStatus, job: bool) -> TowerPrinter {
    let mut state = PrinterState::offline();
    state.status = status;
    state.job = job.then(|| Job {
        name: "benchy.gcode".into(),
        owner: None,
    });
    TowerPrinter {
        id: format!("id-{name}"),
        name: name.into(),
        state,
        watched: true,
        materials: vec![],
        training: None,
        camera: false,
    }
}

struct Pop {
    dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    admin: String,
    _tower: watch::Sender<Vec<TowerPrinter>>,
}

async fn start(printers: Vec<TowerPrinter>) -> Pop {
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
    let (tx, rx) = watch::channel(printers);
    let app = router(AppState::new(db.clone(), Arc::new(Embedded)).with_tower(rx));
    let mut pop = Pop {
        dir,
        db,
        app,
        admin: String::new(),
        _tower: tx,
    };
    let (_, _, set) = pop
        .send(
            "POST",
            "/api/session",
            None,
            Some(json!({"username": "ada", "password": "ada's password"})),
            LOCAL,
        )
        .await;
    pop.admin = set.unwrap();
    pop
}

impl Pop {
    async fn send(
        &self,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        from: &str,
    ) -> (StatusCode, Value, Option<String>) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let mut req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(from.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let set = res
            .headers()
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            code,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            set,
        )
    }

    fn write_events(&self, lines: &[&str]) {
        let path = self.dir.path().join(EVENTS);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.join("\n") + "\n").unwrap();
    }

    async fn audit(&self) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT kind, actor_name, summary FROM audit WHERE actor_name = 'the updater' ORDER BY id",
        )
        .fetch_all(&self.db)
        .await
        .unwrap()
    }
}

/// Busy is any print under way: printing, paused, or stopped mid-print for
/// a person. Not a printer that is idle, done, off, or stopped with no print.
#[test]
fn a_printer_is_busy_while_a_print_is_under_way() {
    let all = vec![
        printer("Printing", PrinterStatus::Printing, true),
        printer("Paused", PrinterStatus::Paused, true),
        printer("Jammed", PrinterStatus::NeedsHand, true),
        printer("Errored idle", PrinterStatus::NeedsHand, false),
        printer("Idle", PrinterStatus::Idle, false),
        printer("Done", PrinterStatus::Done, true),
        printer("Offline", PrinterStatus::Offline, false),
    ];
    assert_eq!(busy(&all), ["Printing", "Paused", "Jammed"]);
}

#[tokio::test]
async fn the_updater_is_told_the_version_and_whether_it_is_quiet_from_this_machine_only() {
    let pop = start(vec![
        printer("Voron 2.4", PrinterStatus::Printing, true),
        printer("MK4", PrinterStatus::Idle, false),
    ])
    .await;
    let (code, body, _) = pop
        .send("GET", "/api/updater/quiet", None, None, LOCAL)
        .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"version": env!("CARGO_PKG_VERSION"), "quiet": false, "busy": ["Voron 2.4"]})
    );
    let (code, _, _) = pop
        .send("GET", "/api/updater/quiet", None, None, REMOTE)
        .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let quiet = start(vec![printer("MK4", PrinterStatus::Done, true)]).await;
    let (_, body, _) = quiet
        .send("GET", "/api/updater/quiet", None, None, LOCAL)
        .await;
    assert_eq!(body["quiet"], true);
}

/// Each line the updater wrote is recorded once, in the audit log and as the
/// last result admins see, whether POP read it once or many times.
#[tokio::test]
async fn what_the_updater_did_is_recorded_once_and_the_last_is_shown() {
    let pop = start(vec![]).await;
    assert_eq!(
        ingest(&pop.db, pop.dir.path()).await,
        Ok(0),
        "no record yet"
    );
    let (_, status, _) = pop
        .send("GET", "/api/admin/updater", Some(&pop.admin), None, LOCAL)
        .await;
    assert_eq!(
        status,
        json!({"running": env!("CARGO_PKG_VERSION"), "last": null})
    );

    let checked = r#"{"id":"1-1","at":100,"kind":"checked","version":null,"summary":"Up to date: v0.1.0 is the newest release"}"#;
    let applied = r#"{"id":"2-1","at":200,"kind":"applied","version":"v0.2.0","summary":"Upgraded from v0.1.0 to v0.2.0"}"#;
    pop.write_events(&[checked, "not json", applied]);
    assert_eq!(ingest(&pop.db, pop.dir.path()).await, Ok(2));
    assert_eq!(
        ingest(&pop.db, pop.dir.path()).await,
        Ok(0),
        "recorded twice"
    );
    let rolled = r#"{"id":"3-1","at":300,"kind":"rolled_back","version":"v0.3.0","summary":"v0.3.0 failed its health check: back to v0.2.0"}"#;
    let unknown = r#"{"id":"4-1","at":300,"kind":"something_new","version":null,"summary":"A newer updater's news"}"#;
    pop.write_events(&[checked, applied, rolled, unknown]);
    assert_eq!(ingest(&pop.db, pop.dir.path()).await, Ok(2));
    assert_eq!(
        pop.audit().await,
        [
            (
                "update_checked".into(),
                "the updater".into(),
                "Up to date: v0.1.0 is the newest release".into()
            ),
            (
                "update_applied".into(),
                "the updater".into(),
                "Upgraded from v0.1.0 to v0.2.0".into()
            ),
            (
                "update_rolled_back".into(),
                "the updater".into(),
                "v0.3.0 failed its health check: back to v0.2.0".into()
            ),
            (
                "other".into(),
                "the updater".into(),
                "A newer updater's news".into()
            ),
        ]
    );
    let (code, status, _) = pop
        .send("GET", "/api/admin/updater", Some(&pop.admin), None, LOCAL)
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        status["last"]["id"],
        Value::Null,
        "ids are the updater's own"
    );
    assert_eq!(status["last"]["at"], 300);
    assert_eq!(
        status["last"]["kind"], "something_new",
        "the newest, by time then id"
    );
    // Admins only.
    let (code, _, _) = pop
        .send("GET", "/api/admin/updater", None, None, LOCAL)
        .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}
