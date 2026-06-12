use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use std::str::FromStr;
use std::time::Duration;

pub async fn connect(database_url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        // SQLite ships with FK enforcement off; without this the ON DELETE
        // CASCADE clauses in the schema never fire.
        .foreign_keys(true)
        // WAL + synchronous=NORMAL: commits append to the WAL and fsync only at
        // checkpoint rather than on every transaction. With the default
        // journal+FULL, each autocommit write fsyncs — which made bulk writes
        // (e.g. saving a floor plan's hundreds of tiles) painfully slow.
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        // Wait rather than instantly erroring if another writer holds the lock.
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePool::connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
