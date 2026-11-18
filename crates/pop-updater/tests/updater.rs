//! The updater against a fake release feed, a fake POP and a fake machine
//! (#148): a good release is applied, a tampered one refused, a failing one
//! rolled back (with its database), nothing applied mid-print or outside
//! its hours, and nothing older ever.
//!
//! What this does not prove: systemd, `pop restore` and `--version` on a
//! real machine (SystemHost), which tools/package-test.sh's systemd check
//! does not reach either; the restore here is POP's own code, run in-process.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use ml_dsa::{Keypair as _, MlDsa65, Signer as _, SigningKey};
use pkcs8::{EncodePublicKey, LineEnding};
use pop_release::Version;
use pop_updater::{Config, Host, Outcome, run};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// ---- the machine ---------------------------------------------------------------------

#[derive(Default)]
struct Machine {
    /// The binary POP runs as, while it runs.
    running: Option<String>,
    minute: u16,
    bam_down: bool,
    /// systemctl restart says it worked, and nothing restarts.
    restart_does_nothing: bool,
    actions: Vec<String>,
    migrations: u32,
}

#[derive(Clone)]
struct FakeHost {
    m: Arc<Mutex<Machine>>,
    binary: PathBuf,
    data_dir: PathBuf,
}

fn version_of(script: &str) -> Option<Version> {
    script
        .lines()
        .find_map(|l| l.strip_prefix("echo pop "))
        .and_then(Version::parse)
}

impl Host for FakeHost {
    fn minute(&self) -> u16 {
        self.m.lock().unwrap().minute
    }

    async fn version(&self, binary: &Path) -> Result<Version, String> {
        let s = std::fs::read_to_string(binary).map_err(|e| e.to_string())?;
        version_of(&s).ok_or_else(|| "no version".to_owned())
    }

    async fn restart(&self) -> Result<(), String> {
        if self.m.lock().unwrap().restart_does_nothing {
            return Ok(());
        }
        self.stop().await?;
        self.start().await
    }

    async fn stop(&self) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.running = None;
        m.actions.push("stop".into());
        Ok(())
    }

    async fn start(&self) -> Result<(), String> {
        let script = std::fs::read_to_string(&self.binary).map_err(|e| e.to_string())?;
        // A release that changes the schema backs up first, as POP does
        // (#147), then changes the database.
        if script.contains("MIGRATES") && !self.data_dir.join("migrated").exists() {
            let n = {
                let mut m = self.m.lock().unwrap();
                m.migrations += 1;
                m.migrations
            };
            let dir = self.data_dir.join("backups");
            std::fs::create_dir_all(&dir).unwrap();
            pop_server::backup::backup(
                &self.data_dir,
                &dir.join(format!("before-schema-99-from-13-{n}.tar.gz")),
            )
            .await
            .map_err(|e| e.to_string())?;
            std::fs::write(self.data_dir.join("migrated"), "by the new release").unwrap();
        }
        let mut m = self.m.lock().unwrap();
        m.actions
            .push(format!("start {}", version_of(&script).unwrap()));
        m.running = Some(script);
        Ok(())
    }

    async fn restore(&self, _binary: &Path, archive: &Path, data_dir: &Path) -> Result<(), String> {
        self.m.lock().unwrap().actions.push("restore".into());
        pop_server::backup::restore(archive, data_dir)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

// ---- POP, as the machine runs it -----------------------------------------------------

async fn quiet(State(m): State<Arc<Mutex<Machine>>>) -> impl IntoResponse {
    let m = m.lock().unwrap();
    match &m.running {
        None => (StatusCode::BAD_GATEWAY, Json(json!({}))),
        Some(s) => {
            let busy: Vec<&str> = if s.contains("BUSY") {
                vec!["Voron 2.4"]
            } else {
                vec![]
            };
            let v = version_of(s).unwrap().to_string();
            (
                StatusCode::OK,
                Json(
                    json!({"version": v.trim_start_matches('v'), "quiet": busy.is_empty(), "busy": busy}),
                ),
            )
        }
    }
}

async fn healthz(State(m): State<Arc<Mutex<Machine>>>) -> impl IntoResponse {
    let m = m.lock().unwrap();
    match &m.running {
        None => (StatusCode::BAD_GATEWAY, Json(json!({}))),
        Some(s) => {
            let db = !s.contains("BAD");
            let checks = json!({"database": {"ok": db}, "bam": {"ok": !m.bam_down}});
            let ok = db && !m.bam_down;
            let code = if ok {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            (code, Json(json!({"ok": ok, "checks": checks})))
        }
    }
}

// ---- the release feed ----------------------------------------------------------------

#[derive(Default)]
struct Feed {
    releases: Vec<Value>,
    files: BTreeMap<String, Vec<u8>>,
    asked: Vec<String>,
}

type SharedFeed = Arc<Mutex<Feed>>;

async fn latest(State(f): State<SharedFeed>) -> impl IntoResponse {
    let mut f = f.lock().unwrap();
    f.asked.push("latest".into());
    match f.releases.iter().rev().find(|r| r["prerelease"] == false) {
        Some(r) => (StatusCode::OK, Json(r.clone())),
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))),
    }
}

async fn list(State(f): State<SharedFeed>) -> impl IntoResponse {
    let mut f = f.lock().unwrap();
    f.asked.push("list".into());
    Json(Value::Array(f.releases.clone()))
}

async fn file(State(f): State<SharedFeed>, UrlPath(name): UrlPath<String>) -> impl IntoResponse {
    let mut f = f.lock().unwrap();
    f.asked.push(name.clone());
    match f.files.get(&name) {
        Some(b) => (StatusCode::OK, b.clone()),
        None => (StatusCode::NOT_FOUND, vec![]),
    }
}

async fn serve(app: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await });
    format!("http://{addr}")
}

// ---- the release keys, made for each test ----------------------------------------------

struct Signer {
    ml: SigningKey<MlDsa65>,
    ed: ed25519_dalek::SigningKey,
}

impl Signer {
    fn new(seed: u8) -> Self {
        Self {
            ml: SigningKey::<MlDsa65>::from_seed(&[seed; 32].into()),
            ed: ed25519_dalek::SigningKey::from_bytes(&[seed.wrapping_add(1); 32]),
        }
    }

    fn write_public(&self, dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("ml-dsa-65.pub"),
            self.ml
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("ed25519.pub"),
            self.ed
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
        )
        .unwrap();
    }

    fn sign(&self, sums: &[u8]) -> (Vec<u8>, Vec<u8>) {
        (
            // Deterministic ML-DSA, empty context: as OpenSSL's verify takes it.
            self.ml.sign(sums).encode().to_vec(),
            self.ed.sign(sums).to_bytes().to_vec(),
        )
    }
}

fn script(version: &str, marks: &str) -> String {
    format!("#!/bin/sh\necho pop {version}\n# {marks}\n")
}

// ---- a tower -------------------------------------------------------------------------

struct Tower {
    _dir: tempfile::TempDir,
    cfg: Config,
    host: FakeHost,
    feed: SharedFeed,
    feed_url: String,
    signer: Signer,
}

async fn tower(running: &str) -> Tower {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let binary = bin.join("pop");
    std::fs::write(&binary, script(running, "GOOD")).unwrap();
    let data_dir = dir.path().join("data");
    // A real database, so a backup and restore are POP's own.
    std::fs::create_dir_all(&data_dir).unwrap();
    pop_server::db::open(&data_dir.join("pop.db"))
        .await
        .unwrap()
        .close()
        .await;
    let m = Arc::new(Mutex::new(Machine {
        minute: 4 * 60,
        ..Machine::default()
    }));
    let host = FakeHost {
        m: m.clone(),
        binary: binary.clone(),
        data_dir: data_dir.clone(),
    };
    host.start().await.unwrap();
    m.lock().unwrap().actions.clear();
    let pop = serve(
        Router::new()
            .route("/api/updater/quiet", get(quiet))
            .route("/healthz", get(healthz))
            .with_state(m),
    )
    .await;
    let feed: SharedFeed = Arc::default();
    let feed_url = serve(
        Router::new()
            .route("/repos/o/r/releases/latest", get(latest))
            .route("/repos/o/r/releases", get(list))
            .route("/files/{name}", get(file))
            .with_state(feed.clone()),
    )
    .await;
    let signer = Signer::new(7);
    signer.write_public(&dir.path().join("keys"));
    let cfg = Config {
        enabled: true,
        hours: "03:00-05:00".into(),
        repo: "o/r".into(),
        api: feed_url.clone(),
        binary,
        keys: dir.path().join("keys"),
        pop,
        data_dir,
        health_wait_secs: 2,
        ..Config::default()
    };
    Tower {
        _dir: dir,
        cfg,
        host,
        feed,
        feed_url,
        signer,
    }
}

/// How a release is spoiled, for the tests that refuse one.
#[derive(Clone, Copy, PartialEq)]
enum Spoil {
    None,
    /// The binary changed after it was signed.
    Binary,
    /// Signed by keys that are not the release keys.
    Keys,
    /// The ML-DSA signature alone is wrong.
    MlDsa,
}

impl Tower {
    fn publish(&self, version: &str, marks: &str, spoil: Spoil, prerelease: bool) {
        let arch = std::env::consts::ARCH;
        let tag = format!("v{version}");
        let name = format!("pop-{tag}-linux-{arch}");
        let binary = script(version, marks).into_bytes();
        let sums = format!("{:x}  {name}\n", Sha256::digest(&binary));
        let (mut ml, ed) = match spoil {
            Spoil::Keys => Signer::new(99).sign(sums.as_bytes()),
            _ => self.signer.sign(sums.as_bytes()),
        };
        if spoil == Spoil::MlDsa {
            ml[0] ^= 1;
        }
        let served = if spoil == Spoil::Binary {
            script(version, &format!("{marks} TAMPERED")).into_bytes()
        } else {
            binary
        };
        let mut f = self.feed.lock().unwrap();
        let mut assets = vec![];
        for (n, bytes) in [
            (name.clone(), served),
            ("SHA256SUMS".into(), sums.into_bytes()),
            ("SHA256SUMS.ml-dsa-65.sig".into(), ml),
            ("SHA256SUMS.ed25519.sig".into(), ed),
        ] {
            // Each release's files apart, as GitHub keeps them.
            let key = format!("{tag}-{n}");
            assets.push(json!({"name": n, "browser_download_url": format!("{}/files/{key}", self.feed_url)}));
            f.files.insert(key, bytes);
        }
        f.releases.push(
            json!({"tag_name": tag, "draft": false, "prerelease": prerelease, "assets": assets}),
        );
    }

    fn installed(&self) -> String {
        std::fs::read_to_string(&self.cfg.binary).unwrap()
    }

    fn running(&self) -> Option<String> {
        self.host.m.lock().unwrap().running.clone()
    }

    fn actions(&self) -> Vec<String> {
        self.host.m.lock().unwrap().actions.clone()
    }

    fn asked(&self) -> Vec<String> {
        self.feed.lock().unwrap().asked.clone()
    }

    fn events(&self) -> Vec<(String, String)> {
        std::fs::read_to_string(self.cfg.data_dir.join("updater/events.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let v: Value = serde_json::from_str(l).unwrap();
                (
                    v["kind"].as_str().unwrap().to_owned(),
                    v["summary"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    async fn run(&self) -> Outcome {
        run(&self.cfg, &self.host).await
    }
}

// ---- the issue's "done when" ---------------------------------------------------------

#[tokio::test]
async fn a_good_release_is_applied_and_the_old_one_kept() {
    let t = tower("0.1.0").await;
    t.publish("0.2.0", "GOOD", Spoil::None, false);
    assert_eq!(t.run().await, Outcome::Applied);
    assert_eq!(t.installed(), script("0.2.0", "GOOD"));
    assert_eq!(t.running(), Some(script("0.2.0", "GOOD")));
    assert_eq!(
        std::fs::read_to_string(t.cfg.binary.with_file_name("pop.previous")).unwrap(),
        script("0.1.0", "GOOD")
    );
    assert_eq!(t.actions(), ["stop", "start v0.2.0"]);
    assert_eq!(
        t.events(),
        [
            (
                "downloaded".into(),
                "v0.2.0 downloaded: both signatures and its SHA-256 check".into()
            ),
            ("applied".into(), "Upgraded from v0.1.0 to v0.2.0".into()),
        ]
    );
    // Next time: up to date, and nothing downloaded again.
    let asked = t.asked().len();
    assert_eq!(t.run().await, Outcome::UpToDate);
    assert_eq!(t.asked().len(), asked + 1, "{:?}", t.asked());
}

/// A tampered release is refused before anything uses it, whichever part is
/// wrong, and not tried again.
#[tokio::test]
async fn a_tampered_release_is_refused_and_not_tried_again() {
    for (spoil, says) in [
        (Spoil::Binary, "is not the file the release signed"),
        (Spoil::Keys, "ML-DSA-65 signature does not check"),
        (Spoil::MlDsa, "ML-DSA-65 signature does not check"),
    ] {
        let t = tower("0.1.0").await;
        t.publish("0.2.0", "GOOD", spoil, false);
        assert_eq!(t.run().await, Outcome::Refused);
        // Untouched: the binary, the service, nothing staged.
        assert_eq!(t.installed(), script("0.1.0", "GOOD"));
        assert!(t.actions().is_empty(), "{:?}", t.actions());
        assert!(!t.cfg.binary.with_file_name("pop.new").exists());
        let events = t.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].0, "refused");
        assert!(events[0].1.contains(says), "{events:?}");
        // Not again: no download, and it says why it waits.
        let asked = t.asked().len();
        assert_eq!(t.run().await, Outcome::UpToDate);
        assert_eq!(
            t.asked().len(),
            asked + 1,
            "downloaded again: {:?}",
            t.asked()
        );
        assert!(
            t.events()[1].1.starts_with("v0.2.0 is not taken (refused:"),
            "{:?}",
            t.events()
        );
        // A newer, good release is.
        t.publish("0.2.1", "GOOD", Spoil::None, false);
        assert_eq!(t.run().await, Outcome::Applied);
    }
}

/// A release that fails its health check is rolled back, and not tried again.
#[tokio::test]
async fn a_failing_release_is_rolled_back() {
    let t = tower("0.1.0").await;
    t.publish("0.2.0", "BAD", Spoil::None, false);
    assert_eq!(t.run().await, Outcome::RolledBack);
    assert_eq!(t.installed(), script("0.1.0", "GOOD"));
    assert_eq!(t.running(), Some(script("0.1.0", "GOOD")));
    assert_eq!(
        t.actions(),
        ["stop", "start v0.2.0", "stop", "start v0.1.0"]
    );
    let last = t.events().last().unwrap().clone();
    assert_eq!(
        last,
        (
            "rolled_back".into(),
            "v0.2.0 failed its health check (its database check fails): back to v0.1.0".into()
        )
    );
    assert_eq!(t.run().await, Outcome::UpToDate, "tried again");
    assert_eq!(t.installed(), script("0.1.0", "GOOD"));
}

/// One that migrated the database first gets the database back too, from
/// the backup it made, and its own is kept beside it.
#[tokio::test]
async fn a_failing_release_that_migrated_gets_the_database_back() {
    let t = tower("0.1.0").await;
    t.publish("0.2.0", "BAD MIGRATES", Spoil::None, false);
    assert_eq!(t.run().await, Outcome::RolledBack);
    let d = &t.cfg.data_dir;
    assert!(!d.join("migrated").exists(), "the migrated database stayed");
    assert!(d.join("pop.db").exists());
    assert_eq!(
        t.actions(),
        ["stop", "start v0.2.0", "stop", "restore", "start v0.1.0"]
    );
    // The failed data is kept beside it; the record and backups came back.
    let aside: Vec<_> = std::fs::read_dir(d.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("data.failed-v0.2.0-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert!(
        d.parent()
            .unwrap()
            .join(&aside[0])
            .join("migrated")
            .exists()
    );
    assert!(d.join("backups").read_dir().unwrap().next().is_some());
    let events = t.events();
    assert_eq!(
        events[0].0, "downloaded",
        "the record before the rollback was kept: {events:?}"
    );
    let (kind, said) = events.last().unwrap();
    assert_eq!(kind, "rolled_back");
    assert!(
        said.contains(", with the database as it was before v0.2.0"),
        "{said}"
    );
}

/// Nothing is applied mid-print: not downloaded, not staged, not restarted.
#[tokio::test]
async fn nothing_is_applied_while_a_print_is_under_way() {
    let t = tower("0.1.0").await;
    // The running POP reports a busy printer.
    std::fs::write(&t.cfg.binary, script("0.1.0", "GOOD BUSY")).unwrap();
    t.host.restart().await.unwrap();
    t.host.m.lock().unwrap().actions.clear();
    t.publish("0.2.0", "GOOD", Spoil::None, false);
    assert_eq!(t.run().await, Outcome::Waiting);
    // First what it asked (only for the release), then what it said.
    assert_eq!(t.asked(), ["latest"], "downloaded mid-print");
    assert!(t.actions().is_empty(), "{:?}", t.actions());
    assert_eq!(
        t.events(),
        [(
            "waiting".into(),
            "v0.2.0 is ready: waiting for Voron 2.4 to finish printing".into()
        )]
    );
    // The print done, it goes in.
    std::fs::write(&t.cfg.binary, script("0.1.0", "GOOD")).unwrap();
    t.host.restart().await.unwrap();
    assert_eq!(t.run().await, Outcome::Applied);
}

#[tokio::test]
async fn nothing_happens_when_off_or_outside_its_hours() {
    let mut t = tower("0.1.0").await;
    t.publish("0.2.0", "GOOD", Spoil::None, false);
    t.cfg.enabled = false;
    assert_eq!(t.run().await, Outcome::Off);
    t.cfg.enabled = true;
    for minute in [2 * 60 + 59, 5 * 60, 12 * 60] {
        t.host.m.lock().unwrap().minute = minute;
        assert_eq!(t.run().await, Outcome::OutsideHours, "{minute}");
    }
    assert!(t.asked().is_empty(), "asked GitHub: {:?}", t.asked());
    assert!(t.events().is_empty());
    assert_eq!(t.installed(), script("0.1.0", "GOOD"));
    // Its hours may run past midnight.
    t.cfg.hours = "23:00-02:00".into();
    t.host.m.lock().unwrap().minute = 30;
    assert_eq!(t.run().await, Outcome::Applied);
}

/// Never older, and never the same: a downgrade is not an upgrade.
#[tokio::test]
async fn an_older_or_the_same_release_is_never_applied() {
    let t = tower("0.3.0").await;
    t.publish("0.2.0", "GOOD", Spoil::None, false);
    assert_eq!(t.run().await, Outcome::UpToDate);
    t.publish("0.3.0", "GOOD", Spoil::None, false);
    assert_eq!(t.run().await, Outcome::UpToDate);
    assert_eq!(t.installed(), script("0.3.0", "GOOD"));
    assert!(t.actions().is_empty());
    assert_eq!(
        t.events().last().unwrap().1,
        "Up to date: v0.3.0 runs, and v0.3.0 is the newest release"
    );
}

/// Pre-releases only when asked for.
#[tokio::test]
async fn a_prerelease_is_taken_only_when_asked_for() {
    let mut t = tower("0.1.0").await;
    t.publish("0.2.0", "GOOD", Spoil::None, true);
    assert_eq!(t.run().await, Outcome::UpToDate);
    t.cfg.prereleases = true;
    assert_eq!(t.run().await, Outcome::Applied);
    assert_eq!(t.installed(), script("0.2.0", "GOOD"));
}

/// A check that failed before the upgrade (BAM! down) is not the new
/// release's doing, and does not roll it back.
#[tokio::test]
async fn a_check_failing_before_and_after_is_not_a_rollback() {
    let t = tower("0.1.0").await;
    t.host.m.lock().unwrap().bam_down = true;
    t.publish("0.2.0", "GOOD", Spoil::None, false);
    assert_eq!(t.run().await, Outcome::Applied);
}

/// When the old release will not come back either, it says so, for a person.
#[tokio::test]
async fn a_rollback_that_does_not_come_back_says_so() {
    let t = tower("0.1.0").await;
    // The old binary itself fails, once put back.
    std::fs::write(&t.cfg.binary, script("0.1.0", "GOOD")).unwrap();
    t.publish("0.2.0", "BAD", Spoil::None, false);
    // Spoil the old one as it is about to be put back.
    let previous = t.cfg.binary.with_file_name("pop.previous");
    let spoiler = {
        let previous = previous.clone();
        // Bounded: a run that never gets this far fails below, not hangs.
        tokio::spawn(async move {
            for _ in 0..2000 {
                if previous.exists() {
                    std::fs::write(&previous, script("0.1.0", "BAD")).unwrap();
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            false
        })
    };
    assert_eq!(t.run().await, Outcome::Failed);
    assert!(spoiler.await.unwrap(), "the old binary was never set aside");
    let (kind, said) = t.events().last().unwrap().clone();
    assert_eq!(kind, "failed");
    assert!(
        said.contains("and v0.1.0 did not come back healthy either"),
        "{said}"
    );
    assert!(said.contains("journalctl -u pop.service"), "{said}");
}

#[test]
fn hours_are_read_strictly_and_may_cross_midnight() {
    use pop_updater::{hours, within};
    assert_eq!(hours("03:00-05:00"), Ok((180, 300)));
    for bad in [
        "3:00-05:00",
        "03:00",
        "25:00-01:00",
        "03:60-04:00",
        "04:00-04:00",
        "03:00-05:00-06:00",
    ] {
        assert!(hours(bad).is_err(), "{bad}");
    }
    let w = hours("03:00-05:00").unwrap();
    assert!(within(w, 180) && within(w, 299) && !within(w, 300) && !within(w, 179));
    let night = hours("23:00-02:00").unwrap();
    assert!(within(night, 23 * 60) && within(night, 0) && within(night, 119));
    assert!(!within(night, 120) && !within(night, 22 * 60 + 59));
}

#[test]
fn a_missing_configuration_is_off_and_a_wrong_one_is_said() {
    let dir = tempfile::tempdir().unwrap();
    let c = Config::load(&dir.path().join("none.toml")).unwrap();
    assert!(!c.enabled);
    let p = dir.path().join("u.toml");
    std::fs::write(&p, "enabled = true\nhours = \"3-5\"\n").unwrap();
    assert!(Config::load(&p).unwrap_err().contains("hours"));
    std::fs::write(&p, "enabeld = true\n").unwrap();
    assert!(Config::load(&p).unwrap_err().contains("enabeld"));
}

/// The old POP still answering, healthy, is not the new one: a restart that
/// did not happen is not an upgrade.
#[tokio::test]
async fn a_restart_that_did_not_happen_is_not_an_upgrade() {
    let t = tower("0.1.0").await;
    t.host.m.lock().unwrap().restart_does_nothing = true;
    t.publish("0.2.0", "GOOD", Spoil::None, false);
    assert_eq!(t.run().await, Outcome::RolledBack);
    let (kind, said) = t.events().last().unwrap().clone();
    assert_eq!(kind, "rolled_back");
    assert!(said.contains("(it says it is 0.1.0)"), "{said}");
    assert_eq!(t.installed(), script("0.1.0", "GOOD"));
}
