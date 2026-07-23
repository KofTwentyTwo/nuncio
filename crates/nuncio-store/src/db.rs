//! SQLite database engine initialization, connection pooling, and migrations.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;
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
    /// Database file corruption detected.
    #[error("database corruption detected: {0}")]
    Corrupted(String),
    /// Recovery salvage operation failed.
    #[error("database recovery error: {0}")]
    RecoveryFailed(String),
    /// Audit chain verification failed.
    #[error("audit chain integrity error: {0}")]
    ChainIntegrityFailed(String),
}

impl DatabaseError {
    /// Returns true if this error indicates database file corruption or un-openable state.
    pub fn is_corrupt(&self) -> bool {
        match self {
            DatabaseError::Corrupted(_) => true,
            DatabaseError::Query(sqlx_err) => crate::recovery::is_sqlite_corruption_error(sqlx_err),
            DatabaseError::PoolCreation(msg) | DatabaseError::Migration(msg) | DatabaseError::RecoveryFailed(msg) => {
                let lower = msg.to_lowercase();
                lower.contains("not a database")
                    || lower.contains("malformed")
                    || lower.contains("corrupt")
                    || lower.contains("sqlite_notadb")
                    || lower.contains("cantopen")
                    || lower.contains("disk i/o error")
            }
            _ => false,
        }
    }
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
        let url = format!("sqlite://{}", path.to_string_lossy());
        let options = SqliteConnectOptions::from_str(&url)
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

    /// Close the underlying connection pool.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Open database at `path` executing pre-flight Stage 1 integrity check.
    /// If corruption is detected, automatically triggers backup isolation and stream recovery salvage.
    pub async fn open(path: &Path) -> Result<(Self, Option<crate::recovery::RecoverySummary>), DatabaseError> {
        let backup_dir = crate::recovery::CorruptedBackupManager::default_backup_dir();
        Self::open_with_backup_dir(path, &backup_dir).await
    }

    /// Open database at `path` specifying a custom backup directory.
    pub async fn open_with_backup_dir(
        path: &Path,
        backup_dir: &Path,
    ) -> Result<(Self, Option<crate::recovery::RecoverySummary>), DatabaseError> {
        match Self::connect_file(path).await {
            Ok(engine) => {
                let is_healthy = engine.check_integrity().await.unwrap_or(false);
                if is_healthy {
                    Ok((engine, None))
                } else {
                    engine.close().await;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    let summary = crate::recovery::SqliteRecoveryEngine::salvage(path, path, backup_dir).await?;
                    let fresh_engine = Self::connect_file(path).await?;
                    Ok((fresh_engine, Some(summary)))
                }
            }
            Err(err) => {
                if err.is_corrupt() {
                    let summary = crate::recovery::SqliteRecoveryEngine::salvage(path, path, backup_dir).await?;
                    let fresh_engine = Self::connect_file(path).await?;
                    Ok((fresh_engine, Some(summary)))
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Check database integrity executing `PRAGMA quick_check(10);`.
    /// Returns `Ok(true)` if integrity check passes ("ok"), or `Ok(false)` if corrupted.
    pub async fn check_integrity(&self) -> Result<bool, DatabaseError> {
        Self::check_integrity_pool(&self.pool).await
    }

    /// Check database integrity on a target connection pool.
    pub async fn check_integrity_pool(pool: &SqlitePool) -> Result<bool, DatabaseError> {
        let res: Result<(String,), sqlx::Error> = sqlx::query_as("PRAGMA quick_check(10);")
            .fetch_one(pool)
            .await;

        match res {
            Ok((val,)) => Ok(val.eq_ignore_ascii_case("ok")),
            Err(err) => {
                if crate::recovery::is_sqlite_corruption_error(&err) {
                    Ok(false)
                } else {
                    Err(DatabaseError::Query(err))
                }
            }
        }
    }

    /// Cryptographic hash-chain audit ledger verification (`verify_chain_integrity()`)
    /// detecting log tampering or corrupted `filter_execution_logs`.
    pub async fn verify_chain_integrity(&self, secret_key: &str) -> Result<bool, DatabaseError> {
        self.verify_execution_log_chain(secret_key).await
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
            CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                email_address TEXT NOT NULL,
                protocol TEXT NOT NULL,
                server_host TEXT NOT NULL,
                server_port INTEGER NOT NULL,
                use_tls INTEGER NOT NULL,
                keyring_secret_key TEXT NOT NULL,
                sync_interval_secs INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS filter_rules (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                priority INTEGER NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                nsql_text TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS filter_conditions (
                id TEXT PRIMARY KEY NOT NULL,
                rule_id TEXT NOT NULL,
                field TEXT NOT NULL,
                operator TEXT NOT NULL,
                value TEXT NOT NULL,
                logical_op TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS filter_actions (
                id TEXT PRIMARY KEY NOT NULL,
                rule_id TEXT NOT NULL,
                action_type TEXT NOT NULL,
                target TEXT
            );

            CREATE TABLE IF NOT EXISTS pending_remote_mutations (
                id TEXT PRIMARY KEY NOT NULL,
                rule_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                mutation_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                retry_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS filter_execution_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rule_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                action_taken TEXT NOT NULL,
                matched_at INTEGER NOT NULL,
                prev_hash TEXT NOT NULL,
                hash TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_filter_rules_priority ON filter_rules(enabled, priority ASC);
            CREATE INDEX IF NOT EXISTS idx_filter_logs_rule ON filter_execution_logs(rule_id, matched_at DESC);
            CREATE INDEX IF NOT EXISTS idx_pending_mutations_status ON pending_remote_mutations(status, created_at ASC);
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
    }

    /// Save an [`nuncio_core::AccountConfig`] to SQLite.
    pub async fn save_account(
        &self,
        config: &nuncio_core::AccountConfig,
    ) -> Result<(), DatabaseError> {
        let protocol_str = serde_json::to_string(&config.protocol).unwrap_or_default();
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO accounts
            (id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&config.id)
        .bind(&config.name)
        .bind(&config.email_address)
        .bind(protocol_str)
        .bind(&config.server_host)
        .bind(config.server_port as i64)
        .bind(if config.use_tls { 1i64 } else { 0i64 })
        .bind(&config.keyring_secret_key)
        .bind(config.sync_interval_secs as i64)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
    }

    /// Query all saved accounts in SQLite.
    #[allow(clippy::type_complexity)]
    pub async fn list_accounts(&self) -> Result<Vec<nuncio_core::AccountConfig>, DatabaseError> {
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            String,
            i64,
        )> = sqlx::query_as(
            r#"
            SELECT id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs
            FROM accounts
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    name,
                    email_address,
                    protocol_str,
                    server_host,
                    server_port,
                    use_tls,
                    keyring_secret_key,
                    sync_interval_secs,
                )| {
                    let protocol = serde_json::from_str(&protocol_str)
                        .unwrap_or(nuncio_core::AccountProtocol::ImapSmtp);
                    nuncio_core::AccountConfig {
                        id,
                        name,
                        email_address,
                        protocol,
                        server_host,
                        server_port: server_port as u16,
                        use_tls: use_tls != 0,
                        imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                        smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                        keyring_secret_key,
                        sync_interval_secs: sync_interval_secs as u64,
                    }
                },
            )
            .collect())
    }

    /// Save an [`nuncio_core::model::Email`] to SQLite (INSERT OR REPLACE).
    pub async fn save_email(&self, email: &nuncio_core::model::Email) -> Result<(), DatabaseError> {
        let enc_plain = email
            .body_plain
            .as_ref()
            .map(|p| crate::cipher::PayloadCipher::encrypt_text_at_rest(p));
        let enc_html = email
            .body_html
            .as_ref()
            .map(|h| crate::cipher::PayloadCipher::encrypt_text_at_rest(h));

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO messages
            (id, account_id, folder_id, subject, sender, recipient, received_at, read_flag, body_plain, body_html)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&email.id)
        .bind(&email.account_id)
        .bind(&email.folder_id)
        .bind(&email.subject)
        .bind(&email.sender)
        .bind(&email.recipient)
        .bind(email.received_at)
        .bind(if email.read { 1i64 } else { 0i64 })
        .bind(&enc_plain)
        .bind(&enc_html)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
    }

    /// Query synced email messages for a specific folder.
    #[allow(clippy::type_complexity)]
    pub async fn list_messages(
        &self,
        folder_id: &str,
        limit: usize,
    ) -> Result<Vec<nuncio_core::model::Email>, DatabaseError> {
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            Option<String>,
            Option<String>,
        )> = sqlx::query_as(
            r#"
            SELECT id, account_id, folder_id, subject, sender, recipient, received_at, read_flag, body_plain, body_html
            FROM messages
            WHERE folder_id = ?
            ORDER BY received_at DESC
            LIMIT ?
            "#,
        )
        .bind(folder_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    account_id,
                    folder_id,
                    subject,
                    sender,
                    recipient,
                    received_at,
                    read_flag,
                    body_plain,
                    body_html,
                )| {
                    let dec_plain =
                        body_plain.map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&p));
                    let dec_html =
                        body_html.map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&h));
                    nuncio_core::model::Email {
                        id,
                        account_id,
                        folder_id,
                        subject,
                        sender,
                        recipient,
                        received_at,
                        read: read_flag != 0,
                        body_plain: dec_plain,
                        body_html: dec_html,
                        attachments: Vec::new(),
                    }
                },
            )
            .collect())
    }

    /// Retrieve a single message by ID.
    #[allow(clippy::type_complexity)]
    pub async fn get_message(
        &self,
        message_id: &str,
    ) -> Result<nuncio_core::model::Email, DatabaseError> {
        let row: (
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            r#"
            SELECT id, account_id, folder_id, subject, sender, recipient, received_at, read_flag, body_plain, body_html
            FROM messages
            WHERE id = ?
            "#,
        )
        .bind(message_id)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let dec_plain = row
            .8
            .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&p));
        let dec_html = row
            .9
            .map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&h));

        Ok(nuncio_core::model::Email {
            id: row.0,
            account_id: row.1,
            folder_id: row.2,
            subject: row.3,
            sender: row.4,
            recipient: row.5,
            received_at: row.6,
            read: row.7 != 0,
            body_plain: dec_plain,
            body_html: dec_html,
            attachments: Vec::new(),
        })
    }

    /// Query available folders with message counts.
    pub async fn list_folders(&self) -> Result<Vec<nuncio_core::model::Folder>, DatabaseError> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            r#"
            SELECT folder_id, COUNT(*) as total, SUM(CASE WHEN read_flag = 0 THEN 1 ELSE 0 END) as unread
            FROM messages
            GROUP BY folder_id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(folder_id, total, unread)| nuncio_core::model::Folder {
                id: folder_id.clone(),
                name: folder_id,
                total_messages: total as usize,
                unread_messages: unread as usize,
            })
            .collect())
    }

    /// Save or replace a [`nuncio_filter::FilterRule`].
    pub async fn save_filter_rule(
        &self,
        rule: &nuncio_filter::FilterRule,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO filter_rules (id, name, priority, enabled, nsql_text, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&rule.id)
        .bind(&rule.name)
        .bind(rule.priority as i64)
        .bind(if rule.enabled { 1i64 } else { 0i64 })
        .bind(&rule.nsql_text)
        .bind(rule.created_at)
        .bind(rule.updated_at)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
    }

    /// Query all active [`nuncio_filter::FilterRule`] records ordered by priority ascending.
    pub async fn list_filter_rules(&self) -> Result<Vec<nuncio_filter::FilterRule>, DatabaseError> {
        let rows: Vec<(String, String, i64, i64, String, i64, i64)> = sqlx::query_as(
            r#"
            SELECT id, name, priority, enabled, nsql_text, created_at, updated_at
            FROM filter_rules
            ORDER BY priority ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let mut rules = Vec::new();
        for (id, name, priority, enabled, nsql_text, created_at, updated_at) in rows {
            if let Ok(mut parsed) = nuncio_filter::NsqlParser::parse_rule(&name, priority as i32, &nsql_text) {
                parsed.id = id;
                parsed.enabled = enabled != 0;
                parsed.created_at = created_at;
                parsed.updated_at = updated_at;
                rules.push(parsed);
            }
        }
        Ok(rules)
    }

    /// Delete a [`nuncio_filter::FilterRule`] by ID.
    pub async fn delete_filter_rule(&self, rule_id: &str) -> Result<(), DatabaseError> {
        sqlx::query("DELETE FROM filter_rules WHERE id = ?")
            .bind(rule_id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        Ok(())
    }

    /// Save a [`nuncio_filter::PendingRemoteMutation`] outbox record.
    pub async fn save_pending_mutation(
        &self,
        item: &nuncio_filter::PendingRemoteMutation,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO pending_remote_mutations
            (id, rule_id, message_id, mutation_type, payload, status, retry_count, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&item.id)
        .bind(&item.rule_id)
        .bind(&item.message_id)
        .bind(&item.mutation_type)
        .bind(&item.payload)
        .bind(&item.status)
        .bind(item.retry_count as i64)
        .bind(item.created_at)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;
        Ok(())
    }

    /// List pending remote mutations awaiting sync processing.
    pub async fn list_pending_mutations(
        &self,
        limit: usize,
    ) -> Result<Vec<nuncio_filter::PendingRemoteMutation>, DatabaseError> {
        let rows: Vec<(String, String, String, String, String, String, i64, i64)> = sqlx::query_as(
            r#"
            SELECT id, rule_id, message_id, mutation_type, payload, status, retry_count, created_at
            FROM pending_remote_mutations
            WHERE status = 'pending'
            ORDER BY created_at ASC
            LIMIT ?
            "#,
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(id, rule_id, message_id, mutation_type, payload, status, retry_count, created_at)| {
                nuncio_filter::PendingRemoteMutation {
                    id,
                    rule_id,
                    message_id,
                    mutation_type,
                    payload,
                    status,
                    retry_count: retry_count as i32,
                    created_at,
                }
            })
            .collect())
    }

    /// Update mutation status and retry count.
    pub async fn update_mutation_status(
        &self,
        id: &str,
        status: &str,
        retry_count: i32,
    ) -> Result<(), DatabaseError> {
        sqlx::query("UPDATE pending_remote_mutations SET status = ?, retry_count = ? WHERE id = ?")
            .bind(status)
            .bind(retry_count as i64)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        Ok(())
    }

    /// Record a cryptographically hash-chained [`nuncio_filter::FilterExecutionLog`].
    pub async fn save_filter_execution_log(
        &self,
        rule_id: &str,
        message_id: &str,
        action_taken: &str,
        secret_key: &str,
    ) -> Result<nuncio_filter::FilterExecutionLog, DatabaseError> {
        let latest_hash: Option<(String,)> = sqlx::query_as(
            "SELECT hash FROM filter_execution_logs ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let prev_hash = latest_hash.map(|r| r.0).unwrap_or_else(|| "GENESIS".to_string());
        let matched_at = chrono::Utc::now().timestamp();
        let hash = compute_log_hash(&prev_hash, rule_id, message_id, action_taken, matched_at, secret_key);

        let id = sqlx::query(
            r#"
            INSERT INTO filter_execution_logs (rule_id, message_id, action_taken, matched_at, prev_hash, hash)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(rule_id)
        .bind(message_id)
        .bind(action_taken)
        .bind(matched_at)
        .bind(&prev_hash)
        .bind(&hash)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?
        .last_insert_rowid();

        Ok(nuncio_filter::FilterExecutionLog {
            id,
            rule_id: rule_id.to_string(),
            message_id: message_id.to_string(),
            action_taken: action_taken.to_string(),
            matched_at,
            prev_hash,
            hash,
        })
    }

    /// List filter execution logs.
    pub async fn list_filter_execution_logs(
        &self,
        limit: usize,
    ) -> Result<Vec<nuncio_filter::FilterExecutionLog>, DatabaseError> {
        let rows: Vec<(i64, String, String, String, i64, String, String)> = sqlx::query_as(
            r#"
            SELECT id, rule_id, message_id, action_taken, matched_at, prev_hash, hash
            FROM filter_execution_logs
            ORDER BY id DESC
            LIMIT ?
            "#,
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(id, rule_id, message_id, action_taken, matched_at, prev_hash, hash)| {
                nuncio_filter::FilterExecutionLog {
                    id,
                    rule_id,
                    message_id,
                    action_taken,
                    matched_at,
                    prev_hash,
                    hash,
                }
            })
            .collect())
    }

    /// Verify cryptographic hash-chain ledger integrity for filter execution logs.
    pub async fn verify_execution_log_chain(&self, secret_key: &str) -> Result<bool, DatabaseError> {
        let rows: Vec<(i64, String, String, String, i64, String, String)> = sqlx::query_as(
            r#"
            SELECT id, rule_id, message_id, action_taken, matched_at, prev_hash, hash
            FROM filter_execution_logs
            ORDER BY id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let mut expected_prev = "GENESIS".to_string();
        for (_id, rule_id, message_id, action_taken, matched_at, prev_hash, hash) in rows {
            if prev_hash != expected_prev {
                return Ok(false);
            }
            let computed = compute_log_hash(&prev_hash, &rule_id, &message_id, &action_taken, matched_at, secret_key);
            if computed != hash {
                return Ok(false);
            }
            expected_prev = hash;
        }

        Ok(true)
    }

    /// Fetch a page of messages using Keyset Chunking (`WHERE id > ? ORDER BY id ASC LIMIT ?`).
    pub async fn get_message_chunk(
        &self,
        last_id: &str,
        limit: usize,
    ) -> Result<Vec<nuncio_core::model::Email>, DatabaseError> {
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT id, account_id, folder_id, subject, sender, recipient, received_at, read_flag, body_plain, body_html FROM messages "
        );
        if !last_id.is_empty() {
            builder.push("WHERE id > ");
            builder.push_bind(last_id);
        }
        builder.push(" ORDER BY id ASC LIMIT ");
        builder.push_bind(limit as i64);

        let query = builder.build_query_as::<(
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            Option<String>,
            Option<String>,
        )>();

        let rows = query.fetch_all(&self.pool).await.map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let dec_plain = r.8.map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&p));
                let dec_html = r.9.map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&h));
                nuncio_core::model::Email {
                    id: r.0,
                    account_id: r.1,
                    folder_id: r.2,
                    subject: r.3,
                    sender: r.4,
                    recipient: r.5,
                    received_at: r.6,
                    read: r.7 != 0,
                    body_plain: dec_plain,
                    body_html: dec_html,
                    attachments: Vec::new(),
                }
            })
            .collect())
    }
}

fn compute_log_hash(
    prev_hash: &str,
    rule_id: &str,
    message_id: &str,
    action_taken: &str,
    matched_at: i64,
    secret_key: &str,
) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret_key.as_bytes())
        .unwrap_or_else(|_| HmacSha256::new_from_slice(b"nuncio_ledger_secret").expect("valid key"));
    let payload = format!("{prev_hash}:{rule_id}:{message_id}:{action_taken}:{matched_at}");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
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

    #[tokio::test]
    async fn test_save_get_list_email_and_folders() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let email = nuncio_core::model::Email {
            id: "msg-db-100".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "INBOX".to_string(),
            subject: "Database Sync Test".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: false,
            body_plain: Some("Plaintext content".to_string()),
            body_html: Some("<p>HTML content</p>".to_string()),
            attachments: Vec::new(),
        };

        engine
            .save_email(&email)
            .await
            .expect("save email succeeds");

        let fetched = engine
            .get_message("msg-db-100")
            .await
            .expect("get message succeeds");
        assert_eq!(fetched.subject, "Database Sync Test");
        assert!(!fetched.read);

        let msgs = engine
            .list_messages("INBOX", 10)
            .await
            .expect("list messages succeeds");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, "msg-db-100");

        let folders = engine.list_folders().await.expect("list folders succeeds");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, "INBOX");
        assert_eq!(folders[0].unread_messages, 1);
    }

    #[tokio::test]
    async fn test_save_and_list_accounts() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let acct = nuncio_core::AccountConfig {
            id: "acct-test-1".to_string(),
            name: "Work Account".to_string(),
            email_address: "work@nuncio.mx".to_string(),
            protocol: nuncio_core::AccountProtocol::ImapSmtp,
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            use_tls: true,
            imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            keyring_secret_key: "nuncio/acct-test-1".to_string(),
            sync_interval_secs: 60,
        };

        engine
            .save_account(&acct)
            .await
            .expect("save account succeeds");
        let accounts = engine
            .list_accounts()
            .await
            .expect("list accounts succeeds");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acct-test-1");
        assert_eq!(accounts[0].email_address, "work@nuncio.mx");
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

    #[tokio::test]
    async fn test_filter_rules_crud_and_execution_log_ledger() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let nsql = "SELECT * FROM emails WHERE subject CONTAINS 'Spam' ACTION DELETE";
        let rule = nuncio_filter::NsqlParser::parse_rule("Spam Filter", 1, nsql).unwrap();

        engine.save_filter_rule(&rule).await.expect("save filter rule");
        let rules = engine.list_filter_rules().await.expect("list filter rules");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "Spam Filter");

        // Hash-chained logs
        let secret = "test_secret_key";
        let log1 = engine.save_filter_execution_log(&rule.id, "msg-100", "DELETE", secret).await.unwrap();
        let log2 = engine.save_filter_execution_log(&rule.id, "msg-101", "DELETE", secret).await.unwrap();

        assert_eq!(log1.prev_hash, "GENESIS");
        assert_eq!(log2.prev_hash, log1.hash);

        let is_valid = engine.verify_execution_log_chain(secret).await.unwrap();
        assert!(is_valid);

        engine.delete_filter_rule(&rule.id).await.unwrap();
        let rules_after = engine.list_filter_rules().await.unwrap();
        assert_eq!(rules_after.len(), 0);
    }

    #[tokio::test]
    async fn test_pending_mutations_and_keyset_chunking() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        for i in 1..=5 {
            let email = nuncio_core::model::Email {
                id: format!("msg-{i:03}"),
                account_id: "acct-1".to_string(),
                folder_id: "inbox".to_string(),
                subject: format!("Subject {i}"),
                sender: "alice@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: 1700000000 + i,
                read: false,
                body_plain: Some("Hello".to_string()),
                body_html: None,
                attachments: Vec::new(),
            };
            engine.save_email(&email).await.unwrap();
        }

        let chunk1 = engine.get_message_chunk("", 3).await.unwrap();
        assert_eq!(chunk1.len(), 3);
        assert_eq!(chunk1[0].id, "msg-001");
        assert_eq!(chunk1[2].id, "msg-003");

        let last_id = &chunk1.last().unwrap().id;
        let chunk2 = engine.get_message_chunk(last_id, 3).await.unwrap();
        assert_eq!(chunk2.len(), 2);
        assert_eq!(chunk2[0].id, "msg-004");
        assert_eq!(chunk2[1].id, "msg-005");

        let mutation = nuncio_filter::OutboxManager::create_mutation("rule-1", "msg-001", "MOVE", Some("Archive".to_string()));
        engine.save_pending_mutation(&mutation).await.unwrap();

        let pending = engine.list_pending_mutations(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, mutation.id);

        engine.update_mutation_status(&mutation.id, "completed", 1).await.unwrap();
        let pending_after = engine.list_pending_mutations(10).await.unwrap();
        assert_eq!(pending_after.len(), 0);
    }
}

