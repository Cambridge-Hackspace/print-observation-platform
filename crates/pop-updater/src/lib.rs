//! Optional automatic upgrades (#148): its own service, so that a POP that
//! will not start can still be put back.
//!
//! Each run (a systemd timer starts one every quarter of an hour):
//!
//! 1. Nothing, unless it is turned on (`enabled`) and within its hours.
//! 2. Asks GitHub for the newest release (only releases, unless
//!    `prereleases`), and stops if it is not newer than what runs. Never
//!    downgrades.
//! 3. Waits while any printer POP watches is busy with a print
//!    (`/api/updater/quiet`).
//! 4. Downloads SHA256SUMS, its two signatures and this machine's binary,
//!    and refuses the release unless both signatures and the binary's
//!    SHA-256 check (crates/pop-release). A refused release is not tried
//!    again.
//! 5. Puts the new binary in, keeping the old one beside it, and restarts
//!    POP, which backs its database up before it migrates it (#147).
//! 6. Waits for the new POP to say its version, with every health check
//!    that passed before the upgrade still passing. If it does not, puts the
//!    old binary back, and, if the new one migrated the database, the
//!    database as it was (the failed one is kept beside it). A release that
//!    failed is not tried again.
//!
//! Each step is written to `<data_dir>/updater/events.jsonl`, which POP
//! records in its audit log (crates/pop-server/src/updater.rs), and to the
//! journal.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

use pop_release::{Keys, Release, Version};
use serde::{Deserialize, Serialize};

/// `/etc/pop/updater.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Off unless turned on.
    pub enabled: bool,
    /// Take pre-releases too.
    pub prereleases: bool,
    /// When an upgrade may go in, in this machine's time zone: `03:00-05:00`.
    /// May run past midnight: `23:00-02:00`.
    pub hours: String,
    /// The GitHub repository releases come from.
    pub repo: String,
    /// GitHub's API.
    pub api: String,
    /// POP's binary.
    pub binary: PathBuf,
    /// The release public keys: `ml-dsa-65.pub` and `ed25519.pub`.
    pub keys: PathBuf,
    /// POP, as this machine reaches it.
    pub pop: String,
    /// POP's systemd unit.
    pub service: String,
    /// POP's data directory.
    pub data_dir: PathBuf,
    /// The account POP runs as, which owns its data directory.
    pub user: String,
    /// POP's configuration file, for `pop restore`.
    pub pop_config: PathBuf,
    /// How long a new release has to come up healthy.
    pub health_wait_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            prereleases: false,
            hours: "03:00-05:00".into(),
            repo: "Cambridge-Hackspace/print-observation-platform".into(),
            api: "https://api.github.com".into(),
            binary: "/usr/local/bin/pop".into(),
            keys: "/usr/local/share/pop/release-keys".into(),
            pop: "http://127.0.0.1:8080".into(),
            service: "pop.service".into(),
            data_dir: "/var/lib/pop".into(),
            user: "pop".into(),
            pop_config: "/etc/pop/pop.toml".into(),
            health_wait_secs: 120,
        }
    }
}

impl Config {
    /// From `path`; the defaults (off) when there is no such file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        let c: Self = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        hours(&c.hours).map_err(|e| format!("{}: hours: {e}", path.display()))?;
        Ok(c)
    }
}

/// `03:00-05:00` as minutes of the day, from and to.
pub fn hours(s: &str) -> Result<(u16, u16), String> {
    let minute = |t: &str| -> Option<u16> {
        let (h, m) = t.trim().split_once(':')?;
        let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
        (h < 24 && m < 60 && t.trim().len() == 5).then_some(h * 60 + m)
    };
    let bad = || format!("{s:?} is not like 03:00-05:00");
    let (a, b) = s.split_once('-').ok_or_else(bad)?;
    let (from, to) = (minute(a).ok_or_else(bad)?, minute(b).ok_or_else(bad)?);
    if from == to {
        return Err(format!("{s:?} starts when it ends"));
    }
    Ok((from, to))
}

/// Whether `minute` of the day is within `from`..`to`, which may run past
/// midnight.
pub fn within((from, to): (u16, u16), minute: u16) -> bool {
    if from < to {
        (from..to).contains(&minute)
    } else {
        minute >= from || minute < to
    }
}

/// What the machine does for the updater: kept apart so the updater is
/// tested without a machine to break.
pub trait Host: Sync {
    /// The minute of the day, in the machine's time zone.
    fn minute(&self) -> u16;
    /// The version `binary --version` says.
    fn version(&self, binary: &Path) -> impl Future<Output = Result<Version, String>> + Send;
    fn restart(&self) -> impl Future<Output = Result<(), String>> + Send;
    fn stop(&self) -> impl Future<Output = Result<(), String>> + Send;
    fn start(&self) -> impl Future<Output = Result<(), String>> + Send;
    /// `binary restore archive` into `data_dir`, which does not exist yet,
    /// leaving it owned by POP's account.
    fn restore(
        &self,
        binary: &Path,
        archive: &Path,
        data_dir: &Path,
    ) -> impl Future<Output = Result<(), String>> + Send;
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not turned on.
    Off,
    /// Turned on, and outside its hours.
    OutsideHours,
    /// Nothing newer, or nothing newer it has not already turned down.
    UpToDate,
    /// A newer release waits for the printers.
    Waiting,
    /// A newer release did not check, and is not used.
    Refused,
    Applied,
    /// The new release failed its health check; the old one is back.
    RolledBack,
    /// Something went wrong that needs a person.
    Failed,
}

/// Where the updater writes what it does, and what it turned down.
struct Record {
    dir: PathBuf,
    n: u32,
}

/// Releases the updater will not try again, with why.
#[derive(Default, Serialize, Deserialize)]
struct Skips {
    skip: BTreeMap<String, String>,
}

impl Record {
    fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("updater"),
            n: 0,
        }
    }

    /// One thing done: to the journal, and a line for POP's audit log.
    fn note(&mut self, kind: &str, version: Option<&Version>, summary: &str) {
        println!("{kind}: {summary}");
        self.n += 1;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let line = serde_json::json!({
            "id": format!("{}-{}-{}", now.as_millis(), std::process::id(), self.n),
            "at": now.as_secs(),
            "kind": kind,
            "version": version.map(|v| v.to_string()),
            "summary": summary,
        });
        if let Err(e) = self.append(&line.to_string()) {
            eprintln!(
                "cannot write the updater's record in {}: {e}",
                self.dir.display()
            );
        }
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("events.jsonl");
        // Kept short: POP records each line once, by its id, so the oldest
        // can go once there are many.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
            let text = std::fs::read_to_string(&path)?;
            let lines: Vec<&str> = text.lines().collect();
            let keep = lines[lines.len().saturating_sub(500)..].join("\n") + "\n";
            std::fs::write(&path, keep)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{line}")
    }

    fn skips(&self) -> Skips {
        std::fs::read_to_string(self.dir.join("state.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn skip(&self, version: &Version, why: &str) {
        let mut s = self.skips();
        s.skip.insert(version.to_string(), why.to_owned());
        let write = std::fs::create_dir_all(&self.dir).and_then(|()| {
            std::fs::write(
                self.dir.join("state.json"),
                serde_json::to_string_pretty(&s).unwrap_or_default(),
            )
        });
        if let Err(e) = write {
            eprintln!("cannot write the updater's state: {e}");
        }
    }
}

/// A release GitHub offers.
struct Offer {
    version: Version,
    /// Its files, by name, with where to download each.
    assets: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// The newest release on offer, if any.
async fn newest(http: &reqwest::Client, cfg: &Config) -> Result<Option<Offer>, String> {
    let url = if cfg.prereleases {
        format!("{}/repos/{}/releases?per_page=20", cfg.api, cfg.repo)
    } else {
        format!("{}/repos/{}/releases/latest", cfg.api, cfg.repo)
    };
    let res = http
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("cannot ask GitHub for releases: {e}"))?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(format!("GitHub answered {} for the releases", res.status()));
    }
    let releases: Vec<GhRelease> = if cfg.prereleases {
        res.json().await
    } else {
        res.json().await.map(|r| vec![r])
    }
    .map_err(|e| format!("GitHub's answer is not a release: {e}"))?;
    Ok(releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            // A tag that is not a plain version is not a release POP takes.
            let v = Version::parse(&r.tag_name).filter(|v| v.to_string() == r.tag_name)?;
            Some(Offer {
                version: v,
                assets: r
                    .assets
                    .into_iter()
                    .map(|a| (a.name, a.browser_download_url))
                    .collect(),
            })
        })
        .max_by_key(|o| o.version))
}

/// At most this much is downloaded for one file.
const MAX_DOWNLOAD: usize = 256 << 20;

async fn download(http: &reqwest::Client, offer: &Offer, name: &str) -> Result<Vec<u8>, String> {
    let url = offer
        .assets
        .get(name)
        .ok_or_else(|| format!("the release has no {name}"))?;
    let res = http
        .get(url)
        .send()
        .await
        .map_err(|e| format!("cannot download {name}: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("downloading {name}: {}", res.status()));
    }
    if res
        .content_length()
        .is_some_and(|n| n > MAX_DOWNLOAD as u64)
    {
        return Err(format!("{name} is too big to be POP"));
    }
    let bytes = res
        .bytes()
        .await
        .map_err(|e| format!("cannot download {name}: {e}"))?;
    if bytes.len() > MAX_DOWNLOAD {
        return Err(format!("{name} is too big to be POP"));
    }
    Ok(bytes.to_vec())
}

#[derive(Deserialize)]
struct Quiet {
    version: String,
    quiet: bool,
    busy: Vec<String>,
}

async fn quiet(http: &reqwest::Client, cfg: &Config) -> Result<Quiet, String> {
    let res = http
        .get(format!("{}/api/updater/quiet", cfg.pop))
        .send()
        .await
        .map_err(|e| format!("POP does not answer: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("POP answered {}", res.status()));
    }
    res.json()
        .await
        .map_err(|e| format!("POP's answer is not one: {e}"))
}

/// Each `/healthz` check by name, passing or not.
async fn health(http: &reqwest::Client, cfg: &Config) -> Result<BTreeMap<String, bool>, String> {
    #[derive(Deserialize)]
    struct Check {
        ok: bool,
    }
    #[derive(Deserialize)]
    struct Health {
        checks: BTreeMap<String, Check>,
    }
    let res = http
        .get(format!("{}/healthz", cfg.pop))
        .send()
        .await
        .map_err(|e| format!("POP does not answer: {e}"))?;
    let h: Health = res
        .json()
        .await
        .map_err(|e| format!("POP's health is not one: {e}"))?;
    Ok(h.checks.into_iter().map(|(k, c)| (k, c.ok)).collect())
}

/// Whether POP answers as `want`, its database working and every check that
/// passed before still passing; waits up to `wait`. A check that failed
/// before (BAM! down, say) is not the new release's doing.
async fn healthy(
    http: &reqwest::Client,
    cfg: &Config,
    want: &Version,
    before: &BTreeMap<String, bool>,
    wait: Duration,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let why = match (quiet(http, cfg).await, health(http, cfg).await) {
            (Err(e), _) | (_, Err(e)) => e,
            (Ok(q), Ok(h)) => {
                let lost: Vec<&String> = before
                    .iter()
                    .filter(|(name, ok)| **ok && h.get(*name) != Some(&true))
                    .map(|(name, _)| name)
                    .collect();
                if Version::parse(&q.version) != Some(*want) {
                    format!("it says it is {}", q.version)
                } else if h.get("database") != Some(&true) {
                    "its database check fails".to_owned()
                } else if let Some(name) = lost.first() {
                    format!("its {name} check fails, though it passed before")
                } else {
                    return Ok(());
                }
            }
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(why);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn backups(data_dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(data_dir.join("backups"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("before-schema-"))
                .collect()
        })
        .unwrap_or_default()
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        if e.file_type()?.is_file() {
            std::fs::copy(e.path(), to.join(e.file_name()))?;
        }
    }
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// One run: see the module's documentation.
pub async fn run<H: Host>(cfg: &Config, host: &H) -> Outcome {
    if !cfg.enabled {
        println!("automatic upgrades are off ([updater] enabled = false)");
        return Outcome::Off;
    }
    let window = match hours(&cfg.hours) {
        Ok(w) => w,
        Err(e) => {
            Record::new(&cfg.data_dir).note("failed", None, &format!("hours: {e}"));
            return Outcome::Failed;
        }
    };
    if !within(window, host.minute()) {
        println!("outside {}: nothing to do", cfg.hours);
        return Outcome::OutsideHours;
    }
    let mut record = Record::new(&cfg.data_dir);
    let fail = |record: &mut Record, v: Option<&Version>, why: String| {
        record.note("failed", v, &why);
        Outcome::Failed
    };
    let http = match reqwest::Client::builder()
        .user_agent(concat!("pop-updater/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(300))
        .build()
    {
        Ok(c) => c,
        Err(e) => return fail(&mut record, None, format!("cannot make a web client: {e}")),
    };
    let running = match host.version(&cfg.binary).await {
        Ok(v) => v,
        Err(e) => {
            return fail(
                &mut record,
                None,
                format!("cannot tell which POP runs: {e}"),
            );
        }
    };
    let offer = match newest(&http, cfg).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            record.note("checked", None, &format!("No release yet: {running} runs"));
            return Outcome::UpToDate;
        }
        Err(e) => return fail(&mut record, None, e),
    };
    let v = offer.version;
    if v <= running {
        record.note(
            "checked",
            Some(&v),
            &format!("Up to date: {running} runs, and {v} is the newest release"),
        );
        return Outcome::UpToDate;
    }
    if let Some(why) = record.skips().skip.get(&v.to_string()) {
        record.note(
            "checked",
            Some(&v),
            &format!("{v} is not taken ({why}): waiting for a newer release"),
        );
        return Outcome::UpToDate;
    }
    match quiet(&http, cfg).await {
        Ok(q) if q.quiet => {}
        Ok(q) => {
            record.note(
                "waiting",
                Some(&v),
                &format!(
                    "{v} is ready: waiting for {} to finish printing",
                    q.busy.join(", ")
                ),
            );
            return Outcome::Waiting;
        }
        Err(e) => {
            return fail(
                &mut record,
                Some(&v),
                format!("cannot ask POP whether it is quiet: {e}"),
            );
        }
    }

    // The release, checked before anything uses it.
    let keys = match (
        std::fs::read_to_string(cfg.keys.join("ml-dsa-65.pub")),
        std::fs::read_to_string(cfg.keys.join("ed25519.pub")),
    ) {
        (Ok(m), Ok(e)) => match Keys::from_pem(&m, &e) {
            Ok(k) => k,
            Err(e) => return fail(&mut record, Some(&v), e.to_string()),
        },
        _ => {
            return fail(
                &mut record,
                Some(&v),
                format!("no release keys at {}", cfg.keys.display()),
            );
        }
    };
    let arch = std::env::consts::ARCH;
    let name = format!("pop-{v}-linux-{arch}");
    let mut files = Vec::new();
    for f in [
        "SHA256SUMS",
        "SHA256SUMS.ml-dsa-65.sig",
        "SHA256SUMS.ed25519.sig",
        &name,
    ] {
        match download(&http, &offer, f).await {
            Ok(b) => files.push(b),
            Err(e) => return fail(&mut record, Some(&v), e),
        }
    }
    let checked: Result<Release, _> =
        pop_release::check(&keys, &v.to_string(), &files[0], &files[1], &files[2])
            .and_then(|r| r.check_binary("pop", arch, &files[3]).map(|()| r));
    if let Err(e) = checked {
        let why = e.to_string();
        record.note("refused", Some(&v), &format!("{v} refused: {why}"));
        record.skip(&v, &format!("refused: {why}"));
        return Outcome::Refused;
    }
    record.note(
        "downloaded",
        Some(&v),
        &format!("{v} downloaded: both signatures and its SHA-256 check"),
    );

    // In, keeping the old one beside it.
    let staged = with_suffix(&cfg.binary, ".new");
    let previous = with_suffix(&cfg.binary, ".previous");
    let write = std::fs::write(&staged, &files[3]).and_then(|()| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
    });
    if let Err(e) = write {
        return fail(
            &mut record,
            Some(&v),
            format!("cannot write {}: {e}", staged.display()),
        );
    }
    match host.version(&staged).await {
        Ok(said) if said == v => {}
        Ok(said) => {
            let _ = std::fs::remove_file(&staged);
            return fail(
                &mut record,
                Some(&v),
                format!("the {v} downloaded says it is {said}"),
            );
        }
        Err(e) => {
            let _ = std::fs::remove_file(&staged);
            return fail(
                &mut record,
                Some(&v),
                format!("the {v} downloaded will not run here: {e}"),
            );
        }
    }
    let before = health(&http, cfg).await.unwrap_or_default();
    let backups_before = backups(&cfg.data_dir);
    if let Err(e) =
        std::fs::rename(&cfg.binary, &previous).and_then(|()| std::fs::rename(&staged, &cfg.binary))
    {
        return fail(&mut record, Some(&v), format!("cannot put {v} in: {e}"));
    }
    let wait = Duration::from_secs(cfg.health_wait_secs);
    let came_up = match host.restart().await {
        Ok(()) => healthy(&http, cfg, &v, &before, wait).await,
        Err(e) => Err(format!("it would not restart: {e}")),
    };
    let why = match came_up {
        Ok(()) => {
            record.note(
                "applied",
                Some(&v),
                &format!("Upgraded from {running} to {v}"),
            );
            return Outcome::Applied;
        }
        Err(why) => why,
    };

    // Back to what ran before.
    let _ = host.stop().await;
    if let Err(e) = std::fs::rename(&previous, &cfg.binary) {
        record.skip(&v, &format!("failed its health check: {why}"));
        return fail(
            &mut record,
            Some(&v),
            format!("{v} failed its health check ({why}), and {running} cannot be put back: {e}"),
        );
    }
    let migrated = backups(&cfg.data_dir)
        .difference(&backups_before)
        .max()
        .cloned();
    let mut also = String::new();
    if let Some(backup) = migrated {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let aside = with_suffix(&cfg.data_dir, &format!(".failed-{v}-{stamp}"));
        let restored = async {
            std::fs::rename(&cfg.data_dir, &aside)
                .map_err(|e| format!("cannot move the data aside: {e}"))?;
            host.restore(
                &cfg.binary,
                &aside.join("backups").join(&backup),
                &cfg.data_dir,
            )
            .await?;
            copy_dir(&aside.join("updater"), &cfg.data_dir.join("updater"))
                .and_then(|()| copy_dir(&aside.join("backups"), &cfg.data_dir.join("backups")))
                .map_err(|e| format!("cannot copy the record and backups back: {e}"))
        }
        .await;
        match restored {
            Ok(()) => {
                also = format!(
                    ", with the database as it was before {v} (its own is kept in {})",
                    aside.display()
                );
            }
            Err(e) => {
                record = Record::new(&cfg.data_dir);
                record.skip(&v, &format!("failed its health check: {why}"));
                return fail(
                    &mut record,
                    Some(&v),
                    format!(
                        "{v} failed its health check ({why}), and the database could not be put back: {e}; the data is in {}",
                        aside.display()
                    ),
                );
            }
        }
    }
    record.skip(&v, &format!("failed its health check: {why}"));
    let back = match host.start().await {
        Ok(()) => healthy(&http, cfg, &running, &before, wait).await,
        Err(e) => Err(format!("it would not start: {e}")),
    };
    match back {
        Ok(()) => {
            record.note(
                "rolled_back",
                Some(&v),
                &format!("{v} failed its health check ({why}): back to {running}{also}"),
            );
            Outcome::RolledBack
        }
        Err(e) => fail(
            &mut record,
            Some(&v),
            format!(
                "{v} failed its health check ({why}), and {running} did not come back healthy either ({e}): see journalctl -u {}",
                cfg.service
            ),
        ),
    }
}

/// The machine itself: systemctl, and POP's own `--version` and `restore`.
pub struct SystemHost {
    pub service: String,
    pub user: String,
    pub pop_config: PathBuf,
}

async fn command(program: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

impl Host for SystemHost {
    fn minute(&self) -> u16 {
        let now = jiff::Zoned::now();
        (now.hour() as u16) * 60 + now.minute() as u16
    }

    async fn version(&self, binary: &Path) -> Result<Version, String> {
        let out = command(&binary.to_string_lossy(), &["--version".as_ref()]).await?;
        // "pop 0.1.0"
        out.split_whitespace()
            .last()
            .and_then(Version::parse)
            .ok_or_else(|| format!("{} --version said {out:?}", binary.display()))
    }

    async fn restart(&self) -> Result<(), String> {
        command("systemctl", &["restart".as_ref(), self.service.as_ref()])
            .await
            .map(|_| ())
    }

    async fn stop(&self) -> Result<(), String> {
        command("systemctl", &["stop".as_ref(), self.service.as_ref()])
            .await
            .map(|_| ())
    }

    async fn start(&self) -> Result<(), String> {
        command("systemctl", &["start".as_ref(), self.service.as_ref()])
            .await
            .map(|_| ())
    }

    async fn restore(&self, binary: &Path, archive: &Path, data_dir: &Path) -> Result<(), String> {
        let out = tokio::process::Command::new(binary)
            .arg("--config")
            .arg(&self.pop_config)
            .arg("restore")
            .arg(archive)
            .env("POP_STORAGE_DATA_DIR", data_dir)
            .output()
            .await
            .map_err(|e| format!("cannot run pop restore: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "pop restore failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let owner = format!("{0}:{0}", self.user);
        command(
            "chown",
            &["-R".as_ref(), owner.as_ref(), data_dir.as_os_str()],
        )
        .await?;
        command("chmod", &["0700".as_ref(), data_dir.as_os_str()])
            .await
            .map(|_| ())
    }
}
