//! POP's database: SQLite through sqlx (ADR 0001), one file beside the
//! stored print files, in WAL mode. `pop backup` copies both (#3).
//!
//! Upgrading (#147): a new POP brings the database up to date as it opens
//! it. When the server opens a database that has data and is behind
//! ([`open_upgrading`]), it backs the database up first and logs where, so a
//! bad upgrade can be undone. A database a newer POP has used is refused,
//! everywhere, with what to do, rather than opened by a POP that does not
//! know its schema.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use sqlx::{
    Connection,
    migrate::Migrator,
    sqlite::{
        SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
    },
};

/// The migrations compiled into this build, from `migrations/`.
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Open (creating it if need be) the database at `path`, and bring its schema
/// up to date.
pub async fn open(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    if let Some(applied) = applied(path).await? {
        refuse_newer(applied)?;
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new().connect_with(options).await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// How many backups taken before an upgrade are kept: the newest.
pub const KEEP_UPGRADE_BACKUPS: usize = 3;

/// The migrations a database at `path` has had applied, newest last; `None`
/// when there is no database there, or one POP has never migrated.
async fn applied(path: &Path) -> Result<Option<Vec<i64>>, sqlx::Error> {
    if !path.exists() {
        return Ok(None);
    }
    let opts =
        SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?.read_only(true);
    let mut conn = SqliteConnection::connect_with(&opts).await?;
    let has: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(&mut conn)
    .await?;
    if has == 0 {
        conn.close().await.ok();
        return Ok(None);
    }
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&mut conn)
            .await?;
    conn.close().await.ok();
    Ok(Some(versions))
}

/// A database a newer POP has migrated is not this POP's to open.
fn refuse_newer(applied: Vec<i64>) -> Result<(), sqlx::Error> {
    let known = newest_known_migration();
    match applied.iter().max() {
        Some(&at) if at > known => Err(sqlx::Error::Configuration(
            format!(
                "this database was last used by a newer POP (schema {at}; this POP knows up to {known}): \
                 run that POP again, or restore the backup it took before upgrading (docs/deployment.md, \"Upgrading\")"
            )
            .into(),
        )),
        _ => Ok(()),
    }
}

/// Open the database in `data_dir` as the server does: as [`open`], but
/// when it has data and this POP has migrations it lacks, back it up first,
/// into `data_dir/backups/`, keeping the newest [`KEEP_UPGRADE_BACKUPS`].
/// Returns the backup taken, if one was.
pub async fn open_upgrading(data_dir: &Path) -> Result<(SqlitePool, Option<PathBuf>), String> {
    let path = data_dir.join(crate::backup::DATABASE);
    let applied = applied(&path).await.map_err(|e| e.to_string())?;
    let mut taken = None;
    if let Some(applied) = applied {
        refuse_newer(applied.clone()).map_err(|e| e.to_string())?;
        let behind = MIGRATOR.iter().any(|m| !applied.contains(&m.version));
        if behind && !applied.is_empty() {
            let from = applied.iter().max().copied().unwrap_or(0);
            let to = newest_known_migration();
            let dir = data_dir.join(UPGRADE_BACKUPS);
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
            let stamp = crate::users::now();
            let out = dir.join(format!("before-schema-{to}-from-{from}-{stamp}.tar.gz"));
            crate::backup::backup(data_dir, &out)
                .await
                .map_err(|e| format!("cannot back the database up before upgrading it: {e}"))?;
            tracing::info!(
                "upgrading the database from schema {from} to {to}; backed up first to {}",
                out.display()
            );
            prune(&dir);
            taken = Some(out);
        }
    }
    let pool = open(&path).await.map_err(|e| e.to_string())?;
    Ok((pool, taken))
}

/// Where backups taken before an upgrade go, in the data directory. Not
/// itself backed up: `pop backup` takes the database and `files/` only.
pub const UPGRADE_BACKUPS: &str = "backups";

/// Keep the newest [`KEEP_UPGRADE_BACKUPS`] backups taken before upgrades.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut ours: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("before-schema-")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    ours.sort();
    let excess = ours.len().saturating_sub(KEEP_UPGRADE_BACKUPS);
    for (_, old) in ours.into_iter().take(excess) {
        if let Err(e) = std::fs::remove_file(&old) {
            tracing::warn!("cannot remove the old backup {}: {e}", old.display());
        }
    }
}

/// The newest migration this build knows. A database with a newer one was
/// made by a newer POP.
pub fn newest_known_migration() -> i64 {
    MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0)
}

/// The newest migration applied, or 0 for none.
pub async fn schema_version(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success")
        .fetch_one(pool)
        .await
}
