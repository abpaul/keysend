use std::time::Duration;

use loco_rs::{config::Database as DatabaseConfig, Result};
use sea_orm::{
    sqlx::sqlite::{SqliteJournalMode, SqliteSynchronous},
    ConnectOptions, Database, DatabaseConnection, DatabaseTransaction, SqliteTransactionMode,
    TransactionOptions, TransactionTrait,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn connect(config: &DatabaseConfig) -> Result<DatabaseConnection> {
    let mut options = ConnectOptions::new(&config.uri);
    options
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .connect_timeout(Duration::from_millis(config.connect_timeout))
        .idle_timeout(Duration::from_millis(config.idle_timeout))
        .sqlx_logging(config.enable_logging)
        .map_sqlx_sqlite_opts(|sqlite| {
            sqlite
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal)
                .busy_timeout(BUSY_TIMEOUT)
                .foreign_keys(true)
                .pragma("cache_size", "-8000")
                .pragma("mmap_size", "134217728")
                .pragma("journal_size_limit", "67108864")
        });
    if let Some(acquire_timeout) = config.acquire_timeout {
        options.acquire_timeout(Duration::from_millis(acquire_timeout));
    }
    Ok(Database::connect(options).await?)
}

/// Starts a transaction that takes the write lock at once. A deferred transaction that reads
/// first and writes later fails with SQLITE_BUSY_SNAPSHOT when another writer commits between
/// the read and the write; the busy timeout does not apply to that failure.
pub async fn begin_write(db: &DatabaseConnection) -> Result<DatabaseTransaction> {
    Ok(db
        .begin_with_options(TransactionOptions {
            sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
            ..Default::default()
        })
        .await?)
}
