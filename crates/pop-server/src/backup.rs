//! Backup and restore (#3): the database and the stored print files, in one
//! archive.
//!
//! A backup is a gzipped tar holding `MANIFEST.json`, `pop.db` and `files/`.
//! The database is copied with SQLite's `VACUUM INTO`, which reads one
//! consistent snapshot even while the server is writing, so a backup never
//! needs the server stopped. The manifest records every file's size and
//! SHA-256, and a restore checks each one before anything is put in place.
//!
//! A restore never overwrites: it refuses a data directory that already holds
//! a database. It unpacks beside the target, checks the archive against its
//! manifest, checks the database's integrity, refuses a schema newer than this
//! build knows, and only then moves the result into place.

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{
    Connection,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};

use crate::db;

/// The archive layout this build writes and reads.
pub const FORMAT: u32 = 1;
const MANIFEST: &str = "MANIFEST.json";
const DATABASE: &str = "pop.db";
/// Stored print files, under the data directory.
pub const FILES_DIR: &str = "files";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub pop_version: String,
    pub schema_version: i64,
    /// Seconds since the Unix epoch.
    pub created: u64,
    /// Every file in the archive but the manifest, by its path in the archive.
    pub files: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug)]
pub struct BackupError(pub String);

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackupError {}

fn fail(msg: impl Into<String>) -> BackupError {
    BackupError(msg.into())
}

fn io_err(what: impl fmt::Display) -> impl FnOnce(io::Error) -> BackupError {
    move |e| fail(format!("{what}: {e}"))
}

/// Write a backup of `data_dir` to `out`, which must not exist yet.
pub async fn backup(data_dir: &Path, out: &Path) -> Result<Manifest, BackupError> {
    if out.exists() {
        return Err(fail(format!(
            "{} already exists; name a new file",
            out.display()
        )));
    }
    let db_path = data_dir.join(DATABASE);
    if !db_path.exists() {
        return Err(fail(format!(
            "there is no database at {}",
            db_path.display()
        )));
    }
    let work =
        tempfile::tempdir_in(parent_of(out)).map_err(io_err("cannot make a working directory"))?;

    // One consistent snapshot, read-only: a backup must never change, or
    // migrate, the database it copies.
    let snapshot = work.path().join(DATABASE);
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))
        .map_err(|e| fail(e.to_string()))?
        .read_only(true);
    let mut conn = SqliteConnection::connect_with(&opts)
        .await
        .map_err(|e| fail(format!("cannot open {}: {e}", db_path.display())))?;
    sqlx::query("VACUUM INTO ?")
        .bind(snapshot.to_string_lossy().into_owned())
        .execute(&mut conn)
        .await
        .map_err(|e| fail(format!("cannot copy the database: {e}")))?;
    let schema_version: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success")
            .fetch_one(&mut conn)
            .await
            .map_err(|e| fail(format!("cannot read the schema version: {e}")))?;
    conn.close().await.ok();

    // What goes in, with its hash, before anything is written.
    let mut files = BTreeMap::new();
    files.insert(DATABASE.to_owned(), entry(&snapshot)?);
    let stored = data_dir.join(FILES_DIR);
    let mut sources = vec![(DATABASE.to_owned(), snapshot.clone())];
    if stored.is_dir() {
        for (rel, path) in walk(&stored)? {
            let name = format!("{FILES_DIR}/{rel}");
            files.insert(name.clone(), entry(&path)?);
            sources.push((name, path));
        }
    }
    let manifest = Manifest {
        format: FORMAT,
        pop_version: env!("CARGO_PKG_VERSION").to_owned(),
        schema_version,
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        files,
    };

    // Written beside the target, then renamed, so `out` is either a whole
    // archive or nothing.
    let partial = work.path().join("archive.tar.gz");
    {
        let gz = flate2::write::GzEncoder::new(
            File::create(&partial).map_err(io_err("cannot write the archive"))?,
            flate2::Compression::default(),
        );
        let mut tar = tar::Builder::new(gz);
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| fail(e.to_string()))?;
        let mut header = tar::Header::new_gnu();
        header.set_size(json.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, MANIFEST, json.as_slice())
            .map_err(io_err("cannot write the archive"))?;
        for (name, path) in &sources {
            tar.append_path_with_name(path, name)
                .map_err(io_err(format!("cannot add {name}")))?;
        }
        tar.into_inner()
            .and_then(|gz| gz.finish())
            .and_then(|mut f| f.flush())
            .map_err(io_err("cannot finish the archive"))?;
    }
    fs::rename(&partial, out).map_err(io_err(format!("cannot write {}", out.display())))?;
    Ok(manifest)
}

/// Restore the backup at `archive` into `data_dir`, which must not already
/// hold a database.
pub async fn restore(archive: &Path, data_dir: &Path) -> Result<Manifest, BackupError> {
    if data_dir.join(DATABASE).exists() {
        return Err(fail(format!(
            "{} already holds a database; restore into an empty directory, or move it aside first",
            data_dir.display()
        )));
    }
    if data_dir.exists()
        && fs::read_dir(data_dir)
            .map_err(io_err(format!("cannot read {}", data_dir.display())))?
            .next()
            .is_some()
    {
        return Err(fail(format!(
            "{} is not empty; restore into an empty directory",
            data_dir.display()
        )));
    }
    let parent = parent_of(data_dir);
    fs::create_dir_all(&parent).map_err(io_err(format!("cannot create {}", parent.display())))?;
    let work = tempfile::tempdir_in(&parent).map_err(io_err("cannot make a working directory"))?;
    let staged = work.path().join("data");
    fs::create_dir(&staged).map_err(io_err("cannot make a working directory"))?;

    // Unpack, by hand rather than tar's unpack, so every path is checked
    // before anything is written for it.
    let mut manifest: Option<Manifest> = None;
    let mut seen = BTreeMap::new();
    let gz = flate2::read::GzDecoder::new(
        File::open(archive).map_err(io_err(format!("cannot open {}", archive.display())))?,
    );
    let mut tar = tar::Archive::new(gz);
    for item in tar.entries().map_err(io_err("not a POP backup"))? {
        let mut item = item.map_err(io_err("the archive is damaged"))?;
        let name = item
            .path()
            .map_err(io_err("the archive is damaged"))?
            .to_string_lossy()
            .into_owned();
        if !item.header().entry_type().is_file() {
            return Err(fail(format!("{name}: only plain files belong in a backup")));
        }
        let rel = safe_path(&name)?;
        if name == MANIFEST {
            let mut buf = Vec::new();
            item.read_to_end(&mut buf)
                .map_err(io_err("the archive is damaged"))?;
            manifest = Some(
                serde_json::from_slice(&buf)
                    .map_err(|e| fail(format!("{MANIFEST} cannot be read: {e}")))?,
            );
            continue;
        }
        let dest = staged.join(&rel);
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).map_err(io_err(format!("cannot create {}", dir.display())))?;
        }
        let mut hasher = Sha256::new();
        let mut out = File::create(&dest).map_err(io_err(format!("cannot write {name}")))?;
        let mut buf = [0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let n = item
                .read(&mut buf)
                .map_err(io_err("the archive is damaged"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(io_err(format!("cannot write {name}")))?;
            size += n as u64;
        }
        seen.insert(
            name,
            Entry {
                size,
                sha256: hex::encode(hasher.finalize()),
            },
        );
    }
    let manifest =
        manifest.ok_or_else(|| fail(format!("not a POP backup: there is no {MANIFEST}")))?;
    if manifest.format != FORMAT {
        return Err(fail(format!(
            "this backup's format is {}; this POP reads format {FORMAT}",
            manifest.format
        )));
    }

    // The archive must hold exactly what its manifest says, byte for byte.
    for (name, want) in &manifest.files {
        match seen.get(name) {
            None => {
                return Err(fail(format!(
                    "{name} is in the manifest but missing from the archive"
                )));
            }
            Some(have) if have != want => {
                return Err(fail(format!(
                    "{name} does not match the manifest: the archive is damaged or altered"
                )));
            }
            Some(_) => {}
        }
    }
    if let Some(extra) = seen.keys().find(|k| !manifest.files.contains_key(*k)) {
        return Err(fail(format!(
            "{extra} is in the archive but not in its manifest"
        )));
    }
    if !seen.contains_key(DATABASE) {
        return Err(fail(format!("not a POP backup: there is no {DATABASE}")));
    }

    // The database itself: sound, and a schema this build can run.
    let newest = db::newest_known_migration();
    if manifest.schema_version > newest {
        return Err(fail(format!(
            "this backup's database is schema {}, newer than this POP's {newest}; restore it with the POP that made it ({}) or newer",
            manifest.schema_version, manifest.pop_version
        )));
    }
    let opts =
        SqliteConnectOptions::from_str(&format!("sqlite://{}", staged.join(DATABASE).display()))
            .map_err(|e| fail(e.to_string()))?
            .read_only(true);
    let mut conn = SqliteConnection::connect_with(&opts)
        .await
        .map_err(|e| fail(format!("the restored database cannot be opened: {e}")))?;
    let check: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut conn)
        .await
        .map_err(|e| fail(format!("the restored database cannot be checked: {e}")))?;
    conn.close().await.ok();
    if check != "ok" {
        return Err(fail(format!(
            "the restored database fails its integrity check: {check}"
        )));
    }

    // Into place. The target is absent or empty, so a rename suffices.
    if data_dir.exists() {
        fs::remove_dir(data_dir)
            .map_err(io_err(format!("cannot replace {}", data_dir.display())))?;
    }
    fs::rename(&staged, data_dir).map_err(io_err(format!(
        "cannot move the restore into {}",
        data_dir.display()
    )))?;
    Ok(manifest)
}

fn parent_of(p: &Path) -> PathBuf {
    match p.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// An archive path, if it stays inside the directory it is unpacked into.
fn safe_path(name: &str) -> Result<PathBuf, BackupError> {
    let p = Path::new(name);
    let ok = !name.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(p.to_path_buf())
    } else {
        Err(fail(format!(
            "{name}: a path that leaves the data directory"
        )))
    }
}

fn entry(path: &Path) -> Result<Entry, BackupError> {
    let mut f = File::open(path).map_err(io_err(format!("cannot read {}", path.display())))?;
    let mut hasher = Sha256::new();
    let size =
        io::copy(&mut f, &mut hasher).map_err(io_err(format!("cannot read {}", path.display())))?;
    Ok(Entry {
        size,
        sha256: hex::encode(hasher.finalize()),
    })
}

/// Every plain file under `root`, as (`a/b.gcode`, its path).
fn walk(root: &Path) -> Result<Vec<(String, PathBuf)>, BackupError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).map_err(io_err(format!("cannot read {}", dir.display())))? {
            let e = e.map_err(io_err(format!("cannot read {}", dir.display())))?;
            let kind = e
                .file_type()
                .map_err(io_err(format!("cannot read {}", e.path().display())))?;
            if kind.is_dir() {
                stack.push(e.path());
            } else if kind.is_file() {
                let rel = e
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| fail("a stored file outside its directory"))?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((rel, e.path()));
            }
        }
    }
    out.sort();
    Ok(out)
}
