//! SQLite database engine initialization, connection pooling, and migrations.

use std::path::Path;
use std::str::FromStr;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use tempfile::TempDir;
use thiserror::Error;

/// Database errors emitted by `nuncio-store`.
#[derive(Error, Debug)]
pub enum DatabaseError {
    /// Failed to open or configure connection pool.
    #[error("failed to create sqlite connection pool: {0}")]
    PoolCreation(String),
    /// Failed to execute schema migration.
    #[error("failed to run database migration: {0}")]
    Migration(String),
    /// Database query execution error.
    #[error("database query execution error: {0}")]
    Query(#[from] sqlx::Error),
}

/// SQLite database storage engine managing WAL connection pools and migrations.
#[derive(Debug, Clone)]
pub struct DatabaseEngine {
    pool: SqlitePool,
}

impl DatabaseEngine {
    /// Maximum concurrent read/write pool size.
    pub const MAX_CONNECTIONS: u32 = 16;

    /// Connect to a local SQLite database file with production WAL pragmas.
    pub async fn connect_file(path: &Path) -> Result<Self, DatabaseError> {
        let path_str = path.to_str().ok_to_null();
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path_str))
            .map_err(|e| DatabaseError::PoolCreation(e.to_string()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_millis(5000))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(Self::MAX_CONNECTIONS)
            .connect_with(options)
            .await
            .map_err(|e| DatabaseError::PoolCreation(e.to_string()))?;

        let engine = Self { pool };
        engine.migrate().await?;
        Ok(engine)
    }

    /// Connect to an isolated ephemeral database for unit and integration testing.
    pub async fn connect_ephemeral() -> Result<(Self, TempDir), DatabaseError> {
        let dir = tempfile::tempdir().map_err(|e| DatabaseError::PoolCreation(e.to_string()))?;
        let db_path = dir.path().join("nuncio_test.sqlite");
        let engine = Self::connect_file(&db_path).await?;
        Ok((engine, dir))
    }

    /// Access the underlying `sqlx::SqlitePool`.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Execute initial database migrations creating core envelope tables.
    pub async fn migrate(&self) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL,
                folder_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                sender TEXT NOT NULL,
                recipient TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                read_flag INTEGER NOT NULL DEFAULT 0,
                body_plain TEXT,
                body_html TEXT
            );

            CREATE TABLE IF NOT EXISTS calendar_events (
                id TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL,
                calendar_id TEXT NOT NULL,
                summary TEXT NOT NULL,
                start_time INTEGER NOT NULL,
                end_time INTEGER NOT NULL,
                rrule TEXT,
                location TEXT
            );
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
    }
}

trait ToNullStr {
    fn ok_to_null(&self) -> &str;
}

impl ToNullStr for Option<&str> {
    fn ok_to_null(&self) -> &str {
        self.unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ephemeral_database_initializes_and_migrates() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral DB created");

        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(engine.pool())
            .await
            .expect("query messages table");

        assert_eq!(row.0, 0);

        let event_row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM calendar_events")
            .fetch_one(engine.pool())
            .await
            .expect("query calendar_events table");

        assert_eq!(event_row.0, 0);
    }

    #[tokio::test]
    async fn insert_and_query_message() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        sqlx::query(
            "INSERT INTO messages (id, account_id, folder_id, subject, sender, recipient, received_at, read_flag, body_plain)
             VALUES ('msg-1', 'acct-1', 'inbox', 'Hello', 'alice@nuncio.mx', 'bob@nuncio.mx', 1700000000, 0, 'Hi Bob')",
        )
        .execute(engine.pool())
        .await
        .unwrap();

        let subject: (String,) = sqlx::query_as("SELECT subject FROM messages WHERE id = 'msg-1'")
            .fetch_one(engine.pool())
            .await
            .unwrap();

        assert_eq!(subject.0, "Hello");
    }

    #[test]
    fn database_error_display() {
        let err = DatabaseError::PoolCreation("failed".to_string());
        assert_eq!(
            err.to_string(),
            "failed to create sqlite connection pool: failed"
        );

        let mig_err = DatabaseError::Migration("mig failed".to_string());
        assert_eq!(
            mig_err.to_string(),
            "failed to run database migration: mig failed"
        );
    }
}
