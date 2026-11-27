//! Prints waiting to be sent (#40): kept on disk beside the database, within
//! limits, each member's own, and deleted with their owner told when they
//! wait too long.
//!
//! Uploads arrive through the slicer's endpoint (#37) and the browser (#41);
//! these tests store through `prints::store`, which both call, so what they
//! prove holds for either. What they do not prove is the upload itself.

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
    api::{Print, PrintFormat, Role, User},
    assets::Embedded,
    audit::Actor,
    backup, config, db,
    prints::{self, StoreError},
    router,
    users::{self, NewAccount},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tower::ServiceExt;

const PEER: &str = "203.0.113.9:5000";

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../pop-gcode/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

struct Account {
    user: User,
    cookie: String,
}

struct Pop {
    dir: tempfile::TempDir,
    db: SqlitePool,
    app: Router,
    limits: config::Prints,
    grace: Account,
    alan: Account,
    ada: Account,
    uploads: std::sync::atomic::AtomicU32,
}

async fn pop_with(limits: config::Prints) -> Pop {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("pop.db")).await.unwrap();
    let account = |role, username: &'static str| {
        let db = db.clone();
        async move {
            let u = users::create(
                &db,
                NewAccount {
                    username,
                    name: username,
                    role,
                    password: "a long enough password",
                },
                &Actor::command_line(),
            )
            .await
            .unwrap();
            let t = users::start_session(&db, &u, false).await.unwrap();
            Account {
                user: User {
                    id: u.id,
                    username: username.into(),
                    name: username.into(),
                    role,
                },
                cookie: format!("pop_session={t}"),
            }
        }
    };
    let grace = account(Role::Member, "grace").await;
    let alan = account(Role::Member, "alan").await;
    let ada = account(Role::Admin, "ada").await;
    let state = AppState::new(db.clone(), Arc::new(Embedded))
        .with_files_dir(dir.path().join(backup::FILES_DIR))
        .with_prints(limits.clone());
    Pop {
        app: router(state),
        dir,
        db,
        limits,
        grace,
        alan,
        ada,
        uploads: 0.into(),
    }
}

async fn pop() -> Pop {
    pop_with(config::Prints::default()).await
}

struct Res {
    code: StatusCode,
    body: Value,
    bytes: Vec<u8>,
    content_type: Option<String>,
    nosniff: bool,
}

impl Pop {
    fn files(&self) -> PathBuf {
        self.dir.path().join(backup::FILES_DIR)
    }

    /// The files kept for prints, by name, sorted.
    fn kept(&self) -> Vec<String> {
        let Ok(d) = std::fs::read_dir(self.files().join(prints::PRINTS_DIR)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = d
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// Upload `bytes` as `name`, for `who`, as an upload endpoint would:
    /// written to a file on the data directory's filesystem, then stored.
    async fn store(&self, who: &Account, name: &str, bytes: &[u8]) -> Result<Print, StoreError> {
        let n = self
            .uploads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let up = self.dir.path().join("uploads");
        std::fs::create_dir_all(&up).unwrap();
        let file = up.join(n.to_string());
        std::fs::write(&file, bytes).unwrap();
        let got = prints::store(
            &self.db,
            &self.files(),
            &self.limits,
            &who.user,
            name,
            &file,
        )
        .await;
        if got.is_ok() {
            assert!(!file.exists(), "the upload was moved, not copied");
        }
        got
    }

    async fn send(&self, method: Method, uri: &str, who: &Account) -> Res {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, &who.cookie)
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(PEER.parse::<SocketAddr>().unwrap()));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let code = res.status();
        let content_type = res
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned());
        let nosniff = res
            .headers()
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .is_some_and(|v| v == "nosniff");
        let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        Res {
            code,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            bytes,
            content_type,
            nosniff,
        }
    }

    async fn audit(&self, kind: &str) -> Vec<(Option<String>, Option<String>, String)> {
        sqlx::query_as(
            "SELECT actor_name, subject_name, summary FROM audit WHERE kind = ? ORDER BY id",
        )
        .bind(kind)
        .fetch_all(&self.db)
        .await
        .unwrap()
    }
}

fn ids(body: &Value) -> Vec<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect()
}

// ---- storing ---------------------------------------------------------------------

#[tokio::test]
async fn a_print_is_kept_whole_with_what_its_slicer_wrote() {
    let pop = pop().await;
    let gcode = fixture("prusaslicer-mk4s.gcode");
    let p = pop
        .store(&pop.grace, "C:\\Users\\grace\\plate.gcode", &gcode)
        .await
        .unwrap();
    assert_eq!(p.filename, "plate.gcode");
    assert_eq!(p.format, PrintFormat::Gcode);
    assert_eq!(p.bytes, gcode.len() as u64);
    assert_eq!(p.expires_at - p.created_at, 14 * 86_400);
    assert_eq!(pop.kept(), vec![p.id.clone()]);
    assert_eq!(
        std::fs::read(pop.files().join("prints").join(&p.id)).unwrap(),
        gcode,
        "kept byte for byte"
    );
    let sha: String = sqlx::query_scalar("SELECT sha256 FROM prints WHERE id = ?")
        .bind(&p.id)
        .fetch_one(&pop.db)
        .await
        .unwrap();
    // Computed here from the fixture, and by hand: sha256sum agrees.
    assert_eq!(sha, hex::encode(Sha256::digest(&gcode)));
    assert_eq!(
        sha,
        "a671f321a21e201a587a1991e0e161efac049104487945022cba6ad070227759"
    );

    let plate = &p.plates[0];
    assert_eq!(plate.index, 1);
    assert_eq!(plate.slicer.as_deref(), Some("PrusaSlicer 2.9.4"));
    assert_eq!(plate.printer_model.as_deref(), Some("MK4S"));
    assert_eq!(plate.filaments[0].material.as_deref(), Some("PLA"));
    assert_eq!(plate.filaments[0].grams, Some(0.96));
    assert!(!plate.has_thumbnail);

    // What is listed is what was stored.
    let mine = pop.send(Method::GET, "/api/prints", &pop.grace).await;
    assert_eq!(mine.code, StatusCode::OK);
    assert_eq!(mine.body, serde_json::to_value([&p]).unwrap());
    assert_eq!(
        pop.audit("print_stored").await,
        [(
            Some("grace".into()),
            Some("grace".into()),
            "grace stored the print \"plate.gcode\"".into()
        )]
    );
}

#[tokio::test]
async fn a_sliced_3mf_is_kept_with_its_pictures() {
    let pop = pop().await;
    let p = pop
        .store(
            &pop.grace,
            "plate.gcode.3mf",
            &fixture("orcaslicer-ender3v2.gcode.3mf"),
        )
        .await
        .unwrap();
    assert_eq!(p.format, PrintFormat::Gcode3mf);
    assert!(p.plates[0].has_thumbnail);
    assert_eq!(pop.kept(), [p.id.clone(), format!("{}.plate-1.png", p.id)]);
    let png = pop
        .send(
            Method::GET,
            &format!("/api/prints/{}/plates/1/thumbnail", p.id),
            &pop.grace,
        )
        .await;
    assert_eq!(png.code, StatusCode::OK);
    assert_eq!(png.content_type.as_deref(), Some("image/png"));
    assert!(png.nosniff);
    // The archive's Metadata/plate_1.png, hashed by hand.
    assert_eq!(
        hex::encode(Sha256::digest(&png.bytes)),
        "8cd273e413b1855971abb0bd6146a5e18455ddffd41653f9a4c010b175a1bfe3"
    );
    let none = pop
        .send(
            Method::GET,
            &format!("/api/prints/{}/plates/2/thumbnail", p.id),
            &pop.grace,
        )
        .await;
    assert_eq!(none.code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn what_is_not_a_print_pop_keeps_is_refused_and_leaves_nothing() {
    let pop = pop().await;
    for (name, bytes) in [
        ("model.stl", &b"solid x\nendsolid x\n"[..]),
        ("model.3mf", &fixture("orcaslicer-ender3v2.gcode.3mf")[..]),
        ("empty.gcode", b""),
        ("not-an-archive.gcode.3mf", b"; generated by PrusaSlicer\n"),
    ] {
        let e = pop.store(&pop.grace, name, bytes).await.unwrap_err();
        assert!(matches!(e, StoreError::Refused(_)), "{name}: {e}");
    }
    assert_eq!(pop.kept(), Vec::<String>::new());
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM prints")
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn a_file_past_the_limit_is_refused() {
    let limits = config::Prints {
        max_file_mb: 1,
        member_mb: 1,
        ..config::Prints::default()
    };
    let pop = pop_with(limits).await;
    let mut big = b"; printer_model = MK4S\n".to_vec();
    big.resize((1 << 20) + 1, b'\n');
    let e = pop.store(&pop.grace, "big.gcode", &big).await.unwrap_err();
    assert!(
        matches!(e, StoreError::Refused(ref m) if m.contains("at most 1 MB")),
        "{e}"
    );
    big.truncate(1 << 20);
    assert!(pop.store(&pop.grace, "fits.gcode", &big).await.is_ok());
}

#[tokio::test]
async fn a_member_may_have_so_many_waiting_and_so_much() {
    let limits = config::Prints {
        max_file_mb: 1,
        member_mb: 1,
        max_waiting: 2,
        ..config::Prints::default()
    };
    let pop = pop_with(limits).await;
    let half = vec![b'\n'; 600 * 1024];
    let small = b"; printer_model = MK4S\n";

    // So much: 600 KiB waiting leaves no room for 600 more within 1 MiB.
    let first = pop.store(&pop.grace, "a.gcode", &half).await.unwrap();
    let e = pop.store(&pop.grace, "b.gcode", &half).await.unwrap_err();
    assert!(matches!(e, StoreError::Full(_)), "{e}");
    assert_eq!(
        pop.kept(),
        vec![first.id.clone()],
        "the refused file is not kept"
    );
    // Another member's room is their own.
    assert!(pop.store(&pop.alan, "b.gcode", &half).await.is_ok());

    // So many: two.
    pop.store(&pop.grace, "c.gcode", small).await.unwrap();
    let e = pop.store(&pop.grace, "d.gcode", small).await.unwrap_err();
    assert!(
        matches!(e, StoreError::Full(ref m) if m.contains("at most 2 prints")),
        "{e}"
    );
    let graces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM prints WHERE user_id = ?")
        .bind(pop.grace.user.id)
        .fetch_one(&pop.db)
        .await
        .unwrap();
    assert_eq!(graces, 2);
}

#[tokio::test]
async fn two_uploads_at_once_cannot_both_take_the_last_place() {
    let limits = config::Prints {
        max_waiting: 1,
        ..config::Prints::default()
    };
    let pop = Arc::new(pop_with(limits).await);
    for round in 0..20 {
        sqlx::query("DELETE FROM prints")
            .execute(&pop.db)
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            pop.store(&pop.grace, "a.gcode", b"; a\n"),
            pop.store(&pop.grace, "b.gcode", b"; b\n"),
        );
        assert_eq!(
            [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(),
            1,
            "round {round}: {a:?} {b:?}"
        );
    }
}

// ---- whose they are ---------------------------------------------------------------

#[tokio::test]
async fn a_member_sees_and_deletes_only_their_own() {
    let pop = pop().await;
    let p = pop
        .store(
            &pop.grace,
            "plate.gcode.3mf",
            &fixture("orcaslicer-ender3v2.gcode.3mf"),
        )
        .await
        .unwrap();
    let one = format!("/api/prints/{}", p.id);
    let png = format!("/api/prints/{}/plates/1/thumbnail", p.id);

    // Alan: nothing listed, and Grace's print not found, as no print is.
    let r = pop.send(Method::GET, "/api/prints", &pop.alan).await;
    assert_eq!(
        (r.code, r.body.clone()),
        (StatusCode::OK, serde_json::json!([]))
    );
    for (m, uri) in [
        (Method::GET, &one),
        (Method::GET, &png),
        (Method::DELETE, &one),
    ] {
        let r = pop.send(m.clone(), uri, &pop.alan).await;
        assert_eq!(r.code, StatusCode::NOT_FOUND, "{m} {uri}");
        assert_eq!(r.body["error"], "you have no such print", "{m} {uri}");
    }
    let r = pop
        .send(
            Method::GET,
            "/api/prints/0123456789abcdef0123456789abcdef",
            &pop.alan,
        )
        .await;
    assert_eq!(
        r.body["error"], "you have no such print",
        "the same answer as for none"
    );
    assert_eq!(pop.kept().len(), 2, "Alan's delete deleted nothing");
    let r = pop.send(Method::GET, "/api/admin/prints", &pop.alan).await;
    assert_eq!(r.code, StatusCode::FORBIDDEN);

    // Grace: her own.
    let r = pop.send(Method::GET, &one, &pop.grace).await;
    assert_eq!(r.body["id"], p.id.as_str());
    assert_eq!(r.body["owner"]["name"], "grace");

    // Ada, an admin: everyone's.
    let r = pop.send(Method::GET, "/api/admin/prints", &pop.ada).await;
    assert_eq!(ids(&r.body), vec![p.id.clone()]);
    let r = pop.send(Method::GET, &png, &pop.ada).await;
    assert_eq!(r.code, StatusCode::OK);
    let r = pop.send(Method::GET, "/api/prints", &pop.ada).await;
    assert_eq!(
        ids(&r.body),
        Vec::<String>::new(),
        "her own list is her own"
    );
}

#[tokio::test]
async fn deleting_a_print_deletes_its_files_and_is_recorded() {
    let pop = pop().await;
    let own = pop
        .store(
            &pop.grace,
            "own.gcode.3mf",
            &fixture("orcaslicer-ender3v2.gcode.3mf"),
        )
        .await
        .unwrap();
    let other = pop
        .store(&pop.grace, "other.gcode", b"; x\n")
        .await
        .unwrap();

    let r = pop
        .send(
            Method::DELETE,
            &format!("/api/prints/{}", own.id),
            &pop.grace,
        )
        .await;
    assert_eq!(r.code, StatusCode::NO_CONTENT);
    let r = pop
        .send(
            Method::DELETE,
            &format!("/api/prints/{}", other.id),
            &pop.ada,
        )
        .await;
    assert_eq!(r.code, StatusCode::NO_CONTENT);

    assert_eq!(pop.kept(), Vec::<String>::new(), "its file and its picture");
    let r = pop.send(Method::GET, "/api/prints", &pop.grace).await;
    assert_eq!(r.body, serde_json::json!([]));
    assert_eq!(
        pop.audit("print_deleted").await,
        [
            (
                Some("grace".into()),
                Some("grace".into()),
                "grace deleted their print \"own.gcode.3mf\"".into()
            ),
            (
                Some("ada".into()),
                Some("grace".into()),
                "ada deleted grace's print \"other.gcode\"".into()
            ),
        ]
    );
    let r = pop
        .send(
            Method::DELETE,
            &format!("/api/prints/{}", own.id),
            &pop.grace,
        )
        .await;
    assert_eq!(r.code, StatusCode::NOT_FOUND, "deleted is gone");
}

// ---- expiry -----------------------------------------------------------------------

#[tokio::test]
async fn a_print_waiting_too_long_is_deleted_and_its_owner_told() {
    let limits = config::Prints {
        keep_days: 1,
        max_waiting: 1,
        ..config::Prints::default()
    };
    let pop = pop_with(limits).await;
    let p = pop
        .store(
            &pop.grace,
            "plate.gcode.3mf",
            &fixture("orcaslicer-ender3v2.gcode.3mf"),
        )
        .await
        .unwrap();
    assert_eq!(p.expires_at, p.created_at + 86_400);

    // Not yet: a second before its time, nothing happens.
    assert_eq!(
        prints::expire(&pop.db, &pop.files(), p.expires_at - 1)
            .await
            .unwrap(),
        0
    );
    assert_eq!(pop.kept().len(), 2);

    assert_eq!(
        prints::expire(&pop.db, &pop.files(), p.expires_at)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        pop.kept(),
        Vec::<String>::new(),
        "its file and its picture are deleted"
    );
    assert_eq!(
        prints::expire(&pop.db, &pop.files(), p.expires_at + 99)
            .await
            .unwrap(),
        0,
        "once"
    );
    assert_eq!(
        pop.audit("print_expired").await,
        [(
            Some("POP, as prints expire".into()),
            Some("grace".into()),
            "grace's print \"plate.gcode.3mf\" waited unsent past its time and was deleted".into()
        )]
    );

    // Grace is told: it is listed, expired, until she dismisses it.
    let r = pop.send(Method::GET, "/api/prints", &pop.grace).await;
    assert_eq!(ids(&r.body), vec![p.id.clone()]);
    assert_eq!(r.body[0]["expired_at"], p.expires_at);
    let r = pop
        .send(
            Method::GET,
            &format!("/api/prints/{}/plates/1/thumbnail", p.id),
            &pop.grace,
        )
        .await;
    assert_eq!(r.code, StatusCode::NOT_FOUND, "its picture went with it");

    // An expired print takes no place: another fits beside its notice.
    assert!(pop.store(&pop.grace, "next.gcode", b"; x\n").await.is_ok());

    // Dismissed, the notice is gone, and no print was deleted by it.
    let r = pop
        .send(Method::DELETE, &format!("/api/prints/{}", p.id), &pop.grace)
        .await;
    assert_eq!(r.code, StatusCode::NO_CONTENT);
    let r = pop.send(Method::GET, "/api/prints", &pop.grace).await;
    assert_eq!(ids(&r.body).len(), 1);
    assert_eq!(r.body[0]["filename"], "next.gcode");
    assert_eq!(pop.audit("print_deleted").await, []);
}

#[tokio::test]
async fn waiting_prints_are_in_a_backup() {
    let pop = pop().await;
    let p = pop
        .store(
            &pop.grace,
            "plate.gcode.3mf",
            &fixture("orcaslicer-ender3v2.gcode.3mf"),
        )
        .await
        .unwrap();
    // An upload still arriving, beside the files directory.
    std::fs::create_dir_all(pop.dir.path().join("uploads")).unwrap();
    std::fs::write(pop.dir.path().join("uploads/arriving"), b"; half a print").unwrap();
    let out = pop.dir.path().join("backup.tar.gz");
    let m = backup::backup(pop.dir.path(), &out).await.unwrap();
    for f in [p.id.clone(), format!("{}.plate-1.png", p.id)] {
        let path = format!("files/prints/{f}");
        assert!(
            m.files.contains_key(&path),
            "{path} in {:?}",
            m.files.keys()
        );
    }
    assert!(
        !m.files.keys().any(|k| k.contains("arriving")),
        "an upload not yet stored is not backed up"
    );
}
