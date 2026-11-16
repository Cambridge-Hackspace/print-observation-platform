//! POP's database: SQLite through sqlx (ADR 0001), one file beside the
//! stored print files, in WAL mode. `pop backup` copies both (#3).

use std::{path::Path, str::FromStr, time::Duration};

use sqlx::{
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions},
};

/// The migrations compiled into this build, from `migrations/`.
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Open (creating it if need be) the database at `path`, and bring its schema
/// up to date.
pub async fn open(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new().connect_with(options).await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
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
