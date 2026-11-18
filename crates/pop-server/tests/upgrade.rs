//! Upgrading (#147): a database from every earlier schema keeps every row
//! when this POP brings it up to date; the server backs it up first, and
//! the backup restores; a database a newer POP has used is refused.
//!
//! "Every earlier schema" is built here from the first N of this build's
//! own migration files, since POP has no tagged release yet to take a
//! database from. Once it has, a database made by the last release belongs
//! beside these.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use pop_server::{backup, db};
use sqlx::{
    Connection, Row,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

/// This build's migration files, in order.
fn migration_files() -> Vec<PathBuf> {
    let mut f: Vec<PathBuf> = fs::read_dir(migrations_dir())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
    f.sort();
    f
}

async fn connect(path: &Path) -> SqliteConnection {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(false);
    SqliteConnection::connect_with(&opts).await.unwrap()
}

/// A database in `dir` at the schema the first `n` migrations make.
async fn at_schema(dir: &Path, n: usize) -> PathBuf {
    let some = tempfile::tempdir().unwrap();
    for f in &migration_files()[..n] {
        fs::copy(f, some.path().join(f.file_name().unwrap())).unwrap();
    }
    let path = dir.join("pop.db");
    let mut conn = connect(&path).await;
    Migrator::new(some.path())
        .await
        .unwrap()
        .run(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    path
}

/// The tables of the database, POP's own bookkeeping aside.
async fn tables(conn: &mut SqliteConnection) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table'
         AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(conn)
    .await
    .unwrap()
}

/// A row in every table that will take one, its required columns filled by
/// type. Some tables' checks refuse made-up values; those are left empty.
async fn seed(conn: &mut SqliteConnection) -> usize {
    let mut seeded = 0;
    for t in tables(conn).await {
        let cols = sqlx::query(&format!("PRAGMA table_info(\"{t}\")"))
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        let mut names = Vec::new();
        let mut values = Vec::new();
        for c in &cols {
            let name: String = c.get("name");
            let ty: String = c.get::<String, _>("type").to_ascii_uppercase();
            let notnull: i64 = c.get("notnull");
            let pk: i64 = c.get("pk");
            let default: Option<String> = c.get("dflt_value");
            if (notnull == 0 && pk == 0) || default.is_some() || (pk == 1 && ty == "INTEGER") {
                continue;
            }
            names.push(format!("\"{name}\""));
            values.push(match ty.as_str() {
                t if t.contains("INT") => "1".to_owned(),
                t if t.contains("BLOB") => "x'00'".to_owned(),
                t if t.contains("REAL") => "1.0".to_owned(),
                _ => format!("'seed-{t}-{name}'"),
            });
        }
        let sql = if names.is_empty() {
            format!("INSERT INTO \"{t}\" DEFAULT VALUES")
        } else {
            format!(
                "INSERT INTO \"{t}\" ({}) VALUES ({})",
                names.join(", "),
                values.join(", ")
            )
        };
        if sqlx::query(&sql).execute(&mut *conn).await.is_ok() {
            seeded += 1;
        }
    }
    seeded
}

async fn counts(path: &Path) -> BTreeMap<String, i64> {
    let mut conn = connect(path).await;
    let mut out = BTreeMap::new();
    for t in tables(&mut conn).await {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{t}\""))
            .fetch_one(&mut conn)
            .await
            .unwrap();
        out.insert(t, n);
    }
    conn.close().await.unwrap();
    out
}

#[tokio::test]
async fn a_database_from_every_earlier_schema_keeps_every_row() {
    let total = migration_files().len();
    assert!(total >= 10, "this build's migrations were found");
    let mut seeded_any = 0;
    for n in 1..total {
        let dir = tempfile::tempdir().unwrap();
        let path = at_schema(dir.path(), n).await;
        let mut conn = connect(&path).await;
        seeded_any += seed(&mut conn).await;
        conn.close().await.unwrap();
        let before = counts(&path).await;
        let (pool, backed_up) = db::open_upgrading(dir.path()).await.unwrap();
        assert_eq!(
            db::schema_version(&pool).await.unwrap(),
            db::newest_known_migration()
        );
        pool.close().await;
        assert!(
            backed_up.is_some(),
            "schema {n}: no backup before upgrading"
        );
        let after = counts(&path).await;
        for (table, rows) in &before {
            assert_eq!(
                after.get(table),
                Some(rows),
                "upgrading from the schema of {n} migrations lost rows of {table}"
            );
        }
    }
    // The seeding did put rows in: the check is not of empty tables.
    assert!(seeded_any > total, "{seeded_any} rows seeded");
}

#[tokio::test]
async fn the_backup_before_an_upgrade_restores_the_database_as_it_was() {
    let total = migration_files().len();
    let dir = tempfile::tempdir().unwrap();
    let path = at_schema(dir.path(), total - 1).await;
    let mut conn = connect(&path).await;
    seed(&mut conn).await;
    conn.close().await.unwrap();
    let before = counts(&path).await;
    let (pool, backed_up) = db::open_upgrading(dir.path()).await.unwrap();
    pool.close().await;
    let backed_up = backed_up.expect("a backup");
    assert!(backed_up.starts_with(dir.path().join(db::UPGRADE_BACKUPS)));
    let name = backed_up
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with(&format!(
            "before-schema-{}-from-{}-",
            db::newest_known_migration(),
            db::newest_known_migration() - 1
        )),
        "{name}"
    );
    // Rolling back: the backup, into an empty directory, is the old schema.
    let back = tempfile::tempdir().unwrap();
    let manifest = backup::restore(&backed_up, back.path()).await.unwrap();
    assert_eq!(manifest.schema_version, db::newest_known_migration() - 1);
    assert_eq!(counts(&back.path().join("pop.db")).await, before);
}

#[tokio::test]
async fn an_up_to_date_or_new_database_is_not_backed_up() {
    let dir = tempfile::tempdir().unwrap();
    let (pool, taken) = db::open_upgrading(dir.path()).await.unwrap();
    pool.close().await;
    assert!(taken.is_none(), "a new database has nothing to keep");
    let (pool, taken) = db::open_upgrading(dir.path()).await.unwrap();
    pool.close().await;
    assert!(taken.is_none(), "an up-to-date one has nothing to change");
    assert!(!dir.path().join(db::UPGRADE_BACKUPS).exists());
}

#[tokio::test]
async fn only_the_newest_backups_before_upgrades_are_kept() {
    let total = migration_files().len();
    let dir = tempfile::tempdir().unwrap();
    let keep = dir.path().join(db::UPGRADE_BACKUPS);
    fs::create_dir_all(&keep).unwrap();
    for i in 0..5 {
        let old = keep.join(format!("before-schema-1-from-0-{i}.tar.gz"));
        fs::write(&old, b"an old backup").unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000 + i);
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }
    // Not ours: left alone.
    fs::write(keep.join("notes.txt"), b"mine").unwrap();
    at_schema(dir.path(), total - 1).await;
    let (pool, taken) = db::open_upgrading(dir.path()).await.unwrap();
    pool.close().await;
    let mut left: Vec<String> = fs::read_dir(&keep)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left.len(), db::KEEP_UPGRADE_BACKUPS + 1, "{left:?}");
    assert!(left.contains(&"notes.txt".to_owned()));
    assert!(
        left.contains(
            &taken
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        )
    );
    assert!(
        left.contains(&"before-schema-1-from-0-4.tar.gz".to_owned()),
        "the newest old ones stay"
    );
    assert!(
        !left.contains(&"before-schema-1-from-0-0.tar.gz".to_owned()),
        "the oldest go"
    );
}

/// Two oracles: open refuses it, saying what to do; and the database is
/// left as it was, not migrated.
#[tokio::test]
async fn a_database_a_newer_pop_used_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pop.db");
    db::open(&path).await.unwrap().close().await;
    let newer = db::newest_known_migration() + 1;
    let mut conn = connect(&path).await;
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)
         VALUES (?, 'from a newer POP', CURRENT_TIMESTAMP, 1, x'00', 0)",
    )
    .bind(newer)
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
    let before = fs::read(&path).unwrap();
    for e in [
        db::open(&path).await.err().map(|e| e.to_string()),
        db::open_upgrading(dir.path()).await.err(),
    ] {
        let e = e.expect("refused");
        assert!(e.contains("newer POP"), "{e}");
        assert!(e.contains(&format!("schema {newer}")), "{e}");
        assert!(e.contains("Upgrading"), "{e}");
    }
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "the newer database was changed"
    );
    assert!(!dir.path().join(db::UPGRADE_BACKUPS).exists());
}

/// Camera logins (#150) make printer_secrets again to widen its check; the
/// seed above cannot fill that table (its check refuses made-up kinds), so a
/// real sealed access code is carried across here, and still opens.
#[tokio::test]
async fn a_sealed_access_code_survives_the_table_being_made_again() {
    const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    let dir = tempfile::tempdir().unwrap();
    // Sealed by this build, on a database of its own.
    let now = db::open(&dir.path().join("now.db")).await.unwrap();
    let secrets = pop_server::secrets::Secrets::new(KEY);
    secrets
        .set_access_code(&now, "p1", "24681357")
        .await
        .unwrap();
    let (nonce, sealed, at): (Vec<u8>, Vec<u8>, i64) =
        sqlx::query_as("SELECT nonce, sealed, updated_at FROM printer_secrets")
            .fetch_one(&now)
            .await
            .unwrap();
    now.close().await;
    // Put in a database from just before the change, which then upgrades.
    let before = migration_files()
        .iter()
        .position(|f| f.to_string_lossy().contains("_camera_logins"))
        .expect("the camera logins migration");
    let old_dir = dir.path().join("old");
    fs::create_dir(&old_dir).unwrap();
    let path = at_schema(&old_dir, before).await;
    let mut conn = connect(&path).await;
    sqlx::query("INSERT INTO printer_secrets (bam_id, kind, nonce, sealed, updated_at) VALUES ('p1', 'access_code', ?, ?, ?)")
        .bind(&nonce)
        .bind(&sealed)
        .bind(at)
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    let upgraded = db::open(&path).await.unwrap();
    assert_eq!(
        secrets
            .access_code(&upgraded, "p1")
            .await
            .unwrap()
            .as_deref(),
        Some("24681357")
    );
    let kept: (Vec<u8>, i64) = sqlx::query_as("SELECT sealed, updated_at FROM printer_secrets")
        .fetch_one(&upgraded)
        .await
        .unwrap();
    assert_eq!(kept, (sealed, at));
    // And the new kind is taken.
    secrets
        .set_camera_login(
            &upgraded,
            "p1",
            &pop_server::secrets::CameraLogin {
                username: "u".into(),
                password: "p".into(),
                origin: "rtsp://10.0.0.40:554".into(),
            },
        )
        .await
        .unwrap();
}
