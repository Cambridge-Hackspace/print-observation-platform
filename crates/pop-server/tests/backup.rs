//! Backup and restore (#3), against real databases on disk.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use pop_server::{
    backup::{self, FILES_DIR},
    db,
};
use sqlx::SqlitePool;

async fn rows(pool: &SqlitePool) -> BTreeMap<String, String> {
    sqlx::query_as::<_, (String, String)>("SELECT key, value FROM settings")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

async fn open(dir: &Path) -> SqlitePool {
    db::open(&dir.join("pop.db")).await.unwrap()
}

/// The issue's "done when": a backup taken while the server is writing
/// restores to a database that passes an integrity check, and the restored
/// data matches.
///
/// The writer commits rows in pairs, one transaction per pair. A copy that is
/// merely readable could hold half a pair; a consistent snapshot never does.
/// And every row it holds must be exactly the row the source holds.
#[tokio::test]
async fn a_backup_taken_while_writing_restores_whole_and_matching() {
    let src = tempfile::tempdir().unwrap();
    let pool = open(src.path()).await;
    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let writer = {
        let (pool, stop, written) = (pool.clone(), stop.clone(), written.clone());
        tokio::spawn(async move {
            let mut i = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let mut tx = pool.begin().await.unwrap();
                for half in ["a", "b"] {
                    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
                        .bind(format!("pair-{i:06}-{half}"))
                        .bind(format!("value {i} {half}"))
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                }
                tx.commit().await.unwrap();
                i += 1;
                written.store(i, Ordering::Relaxed);
            }
        })
    };
    // Let it get going, back up mid-stream, and let it carry on afterwards.
    while written.load(Ordering::Relaxed) < 200 {
        tokio::task::yield_now().await;
    }
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("pop-backup.tar.gz");
    backup::backup(src.path(), &archive).await.unwrap();
    let at_backup = written.load(Ordering::Relaxed);
    while written.load(Ordering::Relaxed) < at_backup + 200 {
        tokio::task::yield_now().await;
    }
    stop.store(true, Ordering::Relaxed);
    writer.await.unwrap();

    let dest = out.path().join("restored");
    backup::restore(&archive, &dest).await.unwrap();
    let restored_pool = open(&dest).await;
    let check: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&restored_pool)
        .await
        .unwrap();
    assert_eq!(check, "ok");

    let source = rows(&pool).await;
    let restored = rows(&restored_pool).await;
    assert!(
        restored.len() >= 400,
        "the backup was taken mid-stream, after 200 pairs: {}",
        restored.len()
    );
    assert!(
        restored.len() < source.len(),
        "the writer carried on after the backup"
    );
    for (k, v) in &restored {
        assert_eq!(source.get(k), Some(v), "{k} differs from the source");
    }
    for k in restored.keys() {
        let other = if k.ends_with("-a") {
            k.replace("-a", "-b")
        } else {
            k.replace("-b", "-a")
        };
        assert!(
            restored.contains_key(&other),
            "{k} was restored without {other}: not one snapshot"
        );
    }
}

#[tokio::test]
async fn stored_files_come_back_byte_for_byte() {
    let src = tempfile::tempdir().unwrap();
    let _pool = open(src.path()).await;
    let files = src.path().join(FILES_DIR);
    fs::create_dir_all(files.join("2026/11")).unwrap();
    let gcode: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    fs::write(files.join("2026/11/benchy.gcode"), &gcode).unwrap();
    fs::write(files.join("note.txt"), "a stored file").unwrap();

    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("b.tar.gz");
    let m = backup::backup(src.path(), &archive).await.unwrap();
    assert!(
        m.files.contains_key("files/2026/11/benchy.gcode"),
        "{:?}",
        m.files.keys()
    );

    let dest = out.path().join("restored");
    backup::restore(&archive, &dest).await.unwrap();
    assert_eq!(
        fs::read(dest.join("files/2026/11/benchy.gcode")).unwrap(),
        gcode
    );
    assert_eq!(
        fs::read_to_string(dest.join("files/note.txt")).unwrap(),
        "a stored file"
    );
}

#[tokio::test]
async fn a_backup_with_the_server_stopped_works_too() {
    let src = tempfile::tempdir().unwrap();
    let pool = open(src.path()).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('k', 'v')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("b.tar.gz");
    backup::backup(src.path(), &archive).await.unwrap();
    let dest = out.path().join("restored");
    backup::restore(&archive, &dest).await.unwrap();
    assert_eq!(
        rows(&open(&dest).await).await.get("k").map(String::as_str),
        Some("v")
    );
}

#[tokio::test]
async fn a_backup_never_overwrites_an_archive() {
    let src = tempfile::tempdir().unwrap();
    let _pool = open(src.path()).await;
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("b.tar.gz");
    fs::write(&archive, "something precious").unwrap();
    let e = backup::backup(src.path(), &archive).await.unwrap_err();
    assert!(e.to_string().contains("already exists"), "{e}");
    assert_eq!(fs::read_to_string(&archive).unwrap(), "something precious");
}

#[tokio::test]
async fn a_restore_never_overwrites_a_database() {
    let src = tempfile::tempdir().unwrap();
    let _pool = open(src.path()).await;
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("b.tar.gz");
    backup::backup(src.path(), &archive).await.unwrap();
    let e = backup::restore(&archive, src.path()).await.unwrap_err();
    assert!(e.to_string().contains("already holds a database"), "{e}");
}

// ---- archives that must be refused ----------------------------------------------

/// Rewrites a good backup's archive, changing what `edit` changes.
fn rewrite(
    archive: &Path,
    edit: impl Fn(&str, Vec<u8>) -> Option<(String, Vec<u8>)>,
    extra: &[(&str, &[u8])],
) {
    let mut entries = Vec::new();
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(
        fs::File::open(archive).unwrap(),
    ));
    for e in tar.entries().unwrap() {
        let mut e = e.unwrap();
        let name = e.path().unwrap().to_string_lossy().into_owned();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut e, &mut data).unwrap();
        if let Some(kept) = edit(&name, data) {
            entries.push(kept);
        }
    }
    for (n, d) in extra {
        entries.push(((*n).to_owned(), d.to_vec()));
    }
    let gz = flate2::write::GzEncoder::new(
        fs::File::create(archive).unwrap(),
        flate2::Compression::fast(),
    );
    let mut b = tar::Builder::new(gz);
    for (name, data) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        // Bypass the builder's own path checks, as a hostile archive would.
        let raw = &mut h.as_old_mut().name;
        raw[..name.len()].copy_from_slice(name.as_bytes());
        h.set_cksum();
        b.append(&h, data.as_slice()).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap().flush().unwrap();
}

async fn good_backup(out: &Path) -> std::path::PathBuf {
    let src = out.join("src");
    fs::create_dir_all(src.join(FILES_DIR)).unwrap();
    let _pool = open(&src).await;
    fs::write(src.join(FILES_DIR).join("a.gcode"), "G28\n").unwrap();
    let archive = out.join("b.tar.gz");
    backup::backup(&src, &archive).await.unwrap();
    archive
}

async fn refused(archive: &Path, dest: &Path) -> String {
    let e = backup::restore(archive, dest)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        !dest.join("pop.db").exists(),
        "a refused restore left a database behind: {e}"
    );
    e
}

#[tokio::test]
async fn an_altered_file_is_refused_by_name() {
    let out = tempfile::tempdir().unwrap();
    let archive = good_backup(out.path()).await;
    rewrite(
        &archive,
        |n, d| {
            Some((
                n.to_owned(),
                if n == "files/a.gcode" {
                    b"G28\nM104 S300\n".to_vec()
                } else {
                    d
                },
            ))
        },
        &[],
    );
    let e = refused(&archive, &out.path().join("r")).await;
    assert!(
        e.contains("files/a.gcode") && e.contains("does not match"),
        "{e}"
    );
}

#[tokio::test]
async fn a_missing_file_is_refused_by_name() {
    let out = tempfile::tempdir().unwrap();
    let archive = good_backup(out.path()).await;
    rewrite(
        &archive,
        |n, d| (n != "files/a.gcode").then(|| (n.to_owned(), d)),
        &[],
    );
    let e = refused(&archive, &out.path().join("r")).await;
    assert!(e.contains("files/a.gcode") && e.contains("missing"), "{e}");
}

#[tokio::test]
async fn a_file_the_manifest_does_not_list_is_refused() {
    let out = tempfile::tempdir().unwrap();
    let archive = good_backup(out.path()).await;
    rewrite(
        &archive,
        |n, d| Some((n.to_owned(), d)),
        &[("files/stowaway.sh", b"rm -rf /")],
    );
    let e = refused(&archive, &out.path().join("r")).await;
    assert!(e.contains("files/stowaway.sh"), "{e}");
}

#[tokio::test]
async fn a_path_that_leaves_the_data_directory_is_refused_before_it_is_written() {
    let out = tempfile::tempdir().unwrap();
    let archive = good_backup(out.path()).await;
    rewrite(
        &archive,
        |n, d| Some((n.to_owned(), d)),
        &[("../escaped.txt", b"out")],
    );
    let e = refused(&archive, &out.path().join("r")).await;
    assert!(e.contains("leaves the data directory"), "{e}");
    assert!(
        !out.path().join("escaped.txt").exists(),
        "the escaping file was written"
    );
}

#[tokio::test]
async fn a_newer_schema_is_refused() {
    let out = tempfile::tempdir().unwrap();
    let archive = good_backup(out.path()).await;
    rewrite(
        &archive,
        |n, d| {
            if n != "MANIFEST.json" {
                return Some((n.to_owned(), d));
            }
            let mut m: serde_json::Value = serde_json::from_slice(&d).unwrap();
            m["schema_version"] = serde_json::json!(db::newest_known_migration() + 1);
            Some((n.to_owned(), serde_json::to_vec(&m).unwrap()))
        },
        &[],
    );
    let e = refused(&archive, &out.path().join("r")).await;
    assert!(e.contains("newer than this POP"), "{e}");
}

#[tokio::test]
async fn a_damaged_database_is_refused() {
    let out = tempfile::tempdir().unwrap();
    let archive = good_backup(out.path()).await;
    // Corrupt the database's pages, and make the manifest agree, so only the
    // integrity check can catch it.
    let mut new_entry = None;
    rewrite(
        &archive,
        |n, mut d| {
            if n == "pop.db" {
                for b in d.iter_mut().skip(4096).take(8192) {
                    *b = 0xA5;
                }
            }
            Some((n.to_owned(), d))
        },
        &[],
    );
    {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(&archive).unwrap(),
        ));
        for e in tar.entries().unwrap() {
            let mut e = e.unwrap();
            if e.path().unwrap().to_string_lossy() == "pop.db" {
                let mut d = Vec::new();
                std::io::Read::read_to_end(&mut e, &mut d).unwrap();
                use sha2::Digest;
                new_entry = Some((d.len() as u64, hex::encode(sha2::Sha256::digest(&d))));
            }
        }
    }
    let (size, sha) = new_entry.unwrap();
    rewrite(
        &archive,
        |n, d| {
            if n != "MANIFEST.json" {
                return Some((n.to_owned(), d));
            }
            let mut m: serde_json::Value = serde_json::from_slice(&d).unwrap();
            m["files"]["pop.db"] = serde_json::json!({"size": size, "sha256": sha});
            Some((n.to_owned(), serde_json::to_vec(&m).unwrap()))
        },
        &[],
    );
    let e = refused(&archive, &out.path().join("r")).await;
    assert!(e.contains("integrity") || e.contains("cannot be"), "{e}");
}
