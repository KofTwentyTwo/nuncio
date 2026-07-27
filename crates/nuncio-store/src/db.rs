//! SQLite database engine initialization, connection pooling, and migrations.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use tempfile::TempDir;
use thiserror::Error;

/// Raw row shape for a pending remote mutation record fetched from SQLite.
type PendingMutationRow = (String, String, String, String, String, String, i64, i64);

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
    /// Cryptographic key material could not be provisioned from the secret vault.
    /// This is a fail-closed error: it is returned instead of ever substituting a
    /// compiled-in default key.
    #[error("cryptographic key provisioning failed: {0}")]
    KeyProvisioning(String),
}

impl DatabaseError {
    /// Returns true if this error indicates GENUINE database file corruption (as opposed to a
    /// transient/operational error such as a pool timeout, a busy/locked database, or a
    /// momentary inability to open the file).
    ///
    /// This is data-safety-critical: `true` here is the sole gate that permits
    /// [`DatabaseEngine::open_with_backup_dir`] to run destructive backup-isolation + salvage
    /// (which deletes the live database file). Only unambiguous corruption signatures are
    /// matched; transient conditions must propagate as ordinary errors instead, so callers can
    /// retry rather than lose data to a passing hiccup.
    pub fn is_corrupt(&self) -> bool {
        match self {
            DatabaseError::Corrupted(_) => true,
            DatabaseError::Query(sqlx_err) => crate::recovery::is_sqlite_corruption_error(sqlx_err),
            DatabaseError::PoolCreation(msg)
            | DatabaseError::Migration(msg)
            | DatabaseError::RecoveryFailed(msg) => {
                let lower = msg.to_lowercase();
                // Deliberately excludes "cantopen" and "disk i/o error": both describe
                // operational conditions (locked file, momentary I/O hiccup) that clear up on
                // retry and must never be classified as corruption.
                lower.contains("not a database")
                    || lower.contains("malformed")
                    || lower.contains("sqlite_notadb")
                    || lower.contains("corrupt")
            }
            _ => false,
        }
    }
}

/// SQLite database storage engine managing WAL connection pools and migrations.
///
/// All cryptographic key material (at-rest storage encryption, WORM audit HMAC, filter
/// execution ledger HMAC) is provisioned once at construction time from a
/// [`crate::vault::SecretManager`]-backed vault and held for the lifetime of the engine.
/// There is intentionally no way to construct a `DatabaseEngine` without a working key
/// source: if the vault cannot supply key material, construction fails closed.
#[derive(Clone)]
pub struct DatabaseEngine {
    pool: SqlitePool,
    storage_key: [u8; 32],
    worm_key: Vec<u8>,
    ledger_key: Vec<u8>,
}

impl std::fmt::Debug for DatabaseEngine {
    /// Manual `Debug` impl that never prints cryptographic key material.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseEngine")
            .field("pool", &self.pool)
            .finish_non_exhaustive()
    }
}

/// Resolve the storage/WORM/ledger key bundle for a `DatabaseEngine` from `secrets`,
/// generating and persisting fresh CSPRNG-sourced key material on first use. Fails
/// closed (returns `Err`) rather than ever substituting a compiled-in default.
#[allow(clippy::type_complexity)]
fn resolve_engine_keys(
    secrets: &crate::vault::SecretManager,
) -> Result<([u8; 32], Vec<u8>, Vec<u8>), DatabaseError> {
    let storage_key_bytes = secrets
        .get_or_create_key_bytes(crate::vault::STORAGE_KEY_ACCOUNT, 32)
        .map_err(|e| DatabaseError::KeyProvisioning(e.to_string()))?;
    let storage_key: [u8; 32] = storage_key_bytes.try_into().map_err(|_| {
        DatabaseError::KeyProvisioning("storage key material must be exactly 32 bytes".to_string())
    })?;
    let worm_key = secrets
        .get_or_create_key_bytes(crate::vault::WORM_KEY_ACCOUNT, 32)
        .map_err(|e| DatabaseError::KeyProvisioning(e.to_string()))?;
    let ledger_key = secrets
        .get_or_create_key_bytes(crate::vault::LEDGER_KEY_ACCOUNT, 32)
        .map_err(|e| DatabaseError::KeyProvisioning(e.to_string()))?;
    Ok((storage_key, worm_key, ledger_key))
}

/// Structured result of re-verifying the entire persisted WORM audit
/// ledger's hash chain (backlog story 2.B, GH #172). See
/// [`DatabaseEngine::verify_worm_audit_chain_report`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WormChainReport {
    /// Whether every persisted record's HMAC and inter-record chain linkage
    /// verified.
    pub valid: bool,
    /// Total number of records checked.
    pub record_count: usize,
    /// Sequence number of the first record that failed to verify, if any.
    pub first_broken_seq: Option<u64>,
}

impl DatabaseEngine {
    /// Maximum concurrent read/write pool size.
    pub const MAX_CONNECTIONS: u32 = 16;

    /// Connect to a local SQLite database file with production WAL pragmas, provisioning
    /// cryptographic key material from `secrets`.
    pub async fn connect_file(
        path: &Path,
        secrets: &crate::vault::SecretManager,
    ) -> Result<Self, DatabaseError> {
        let (storage_key, worm_key, ledger_key) = resolve_engine_keys(secrets)?;

        // Ensure the database's parent directory exists before opening. SQLite's
        // `create_if_missing` creates the file but NOT its parent directory, so a
        // fresh install (e.g. `~/.nuncio` not yet created) would otherwise fail to
        // start with SQLITE_CANTOPEN (code 14). Found via dogfooding.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| DatabaseError::PoolCreation(e.to_string()))?;
            }
        }

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

        let engine = Self {
            pool,
            storage_key,
            worm_key,
            ledger_key,
        };
        engine.migrate().await?;
        Ok(engine)
    }

    /// Close the underlying connection pool.
    ///
    /// Forces a full WAL checkpoint (`PRAGMA wal_checkpoint(TRUNCATE)`) before closing so every
    /// committed page is flushed into the main database file and the `-wal` file is truncated.
    /// Without this, a WAL-mode connection can leave committed pages un-flushed in the `-wal`
    /// file; SQLite then performs its own "last connection closes" checkpoint internally, but
    /// that teardown is not guaranteed to complete synchronously within this call on every
    /// platform, and can instead complete moments later on a background thread -- which then
    /// races any code that inspects or replaces the main database file shortly after `close()`
    /// returns (this was the confirmed root cause of a nondeterministic corruption-detection
    /// test failure: a deferred checkpoint silently overwrote a deliberately corrupted header
    /// with the last-known-good page 1 a few milliseconds after `close()` had already
    /// returned). Explicitly checkpointing first leaves nothing for that deferred teardown to
    /// apply, closing the race.
    pub async fn close(&self) {
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE);")
            .execute(&self.pool)
            .await;
        self.pool.close().await;
    }

    /// Open database at `path` executing pre-flight Stage 1 integrity check, provisioning
    /// cryptographic key material from `secrets`.
    /// If corruption is detected, automatically triggers backup isolation and stream recovery salvage.
    pub async fn open(
        path: &Path,
        secrets: &crate::vault::SecretManager,
    ) -> Result<(Self, Option<crate::recovery::RecoverySummary>), DatabaseError> {
        let backup_dir = crate::recovery::CorruptedBackupManager::default_backup_dir();
        Self::open_with_backup_dir(path, &backup_dir, secrets).await
    }

    /// Open database at `path` specifying a custom backup directory, provisioning
    /// cryptographic key material from `secrets`.
    ///
    /// # Data safety
    ///
    /// This is the sole entry point that may trigger destructive backup-isolation + salvage
    /// (which deletes the live database file, keeping only what could be re-extracted). That
    /// path is gated on GENUINE corruption only -- classified via a connection-independent raw
    /// header-byte check (see [`crate::recovery::has_corrupted_sqlite_header`]) and/or
    /// [`DatabaseError::is_corrupt`] / [`crate::recovery::is_sqlite_corruption_error`]. A
    /// transient/operational error (pool timeout, busy, locked, momentarily unable to open)
    /// is NEVER coerced into "corrupt": it is propagated as-is so the caller can retry, and the
    /// live database is left completely untouched.
    pub async fn open_with_backup_dir(
        path: &Path,
        backup_dir: &Path,
        secrets: &crate::vault::SecretManager,
    ) -> Result<(Self, Option<crate::recovery::RecoverySummary>), DatabaseError> {
        // Deterministic, connection-independent pre-flight check: read the raw SQLite header
        // magic bytes directly from disk before ever touching a connection pool. Unlike a
        // `PRAGMA quick_check` run over a freshly (re)opened pooled connection, this cannot race
        // a WAL checkpoint or a connection-close from a prior engine instance still completing
        // in the background, so it authoritatively and reproducibly detects a genuinely
        // corrupted header.
        let header_corrupted = crate::recovery::has_corrupted_sqlite_header(path);

        match Self::connect_file(path, secrets).await {
            Ok(engine) => {
                if header_corrupted {
                    return Self::close_and_salvage(engine, path, backup_dir, secrets).await;
                }
                match engine.check_integrity().await {
                    Ok(true) => Ok((engine, None)),
                    Ok(false) => Self::close_and_salvage(engine, path, backup_dir, secrets).await,
                    Err(err) => {
                        // The integrity probe itself failed with an error that is NOT a
                        // recognized corruption signature (e.g. a pool acquire timeout, a
                        // transient SQLITE_BUSY/locked condition). This is an operational
                        // hiccup, not proof of corruption -- NEVER salvage or delete the live
                        // database on the strength of it. Surface the error and let the caller
                        // retry.
                        engine.close().await;
                        Err(err)
                    }
                }
            }
            Err(err) => {
                if header_corrupted || err.is_corrupt() {
                    let summary = crate::recovery::SqliteRecoveryEngine::salvage(
                        path, path, backup_dir, secrets,
                    )
                    .await?;
                    let fresh_engine = Self::connect_file(path, secrets).await?;
                    Ok((fresh_engine, Some(summary)))
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Close an engine that has been confirmed corrupted (either by a failed integrity check or
    /// by the independent raw-header-byte check), then run backup isolation and stream salvage.
    async fn close_and_salvage(
        engine: Self,
        path: &Path,
        backup_dir: &Path,
        secrets: &crate::vault::SecretManager,
    ) -> Result<(Self, Option<crate::recovery::RecoverySummary>), DatabaseError> {
        engine.close().await;
        // Give the OS a brief moment to release file handles/locks from the just-closed pool
        // before salvage copies and then deletes the live database file.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let summary =
            crate::recovery::SqliteRecoveryEngine::salvage(path, path, backup_dir, secrets).await?;
        let fresh_engine = Self::connect_file(path, secrets).await?;
        Ok((fresh_engine, Some(summary)))
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
    /// detecting log tampering or corrupted `filter_execution_logs`, using the ledger
    /// HMAC key provisioned for this engine.
    pub async fn verify_chain_integrity(&self) -> Result<bool, DatabaseError> {
        self.verify_execution_log_chain().await
    }

    /// Connect to an isolated ephemeral database for unit and integration testing (and for
    /// the current single-process CLI/MCP shells, which do not yet persist across runs).
    /// Cryptographic key material is generated fresh via [`crate::vault::MockKeyring`] for
    /// the lifetime of the temporary database, so this NEVER touches the real OS keyring —
    /// safe to call from headless CI.
    pub async fn connect_ephemeral() -> Result<(Self, TempDir), DatabaseError> {
        let dir = tempfile::tempdir().map_err(|e| DatabaseError::PoolCreation(e.to_string()))?;
        let db_path = dir.path().join("nuncio_test.sqlite");
        let secrets = crate::vault::SecretManager::mock();
        let engine = Self::connect_file(&db_path, &secrets).await?;
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
                sync_interval_secs INTEGER NOT NULL,
                smtp_host TEXT,
                smtp_port INTEGER
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

            CREATE TABLE IF NOT EXISTS worm_audit_records (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_ns INTEGER NOT NULL,
                actor TEXT NOT NULL,
                action TEXT NOT NULL,
                data_hash TEXT NOT NULL,
                previous_block_hash TEXT NOT NULL,
                record_hmac TEXT NOT NULL
            );

            CREATE TRIGGER IF NOT EXISTS prevent_worm_audit_update
            BEFORE UPDATE ON worm_audit_records
            BEGIN
                SELECT RAISE(ABORT, 'WORM audit records are immutable and cannot be updated');
            END;

            CREATE TRIGGER IF NOT EXISTS prevent_worm_audit_delete
            BEFORE DELETE ON worm_audit_records
            BEGIN
                SELECT RAISE(ABORT, 'WORM audit records are immutable and cannot be deleted');
            END;

            CREATE INDEX IF NOT EXISTS idx_filter_rules_priority ON filter_rules(enabled, priority ASC);
            CREATE INDEX IF NOT EXISTS idx_filter_logs_rule ON filter_execution_logs(rule_id, matched_at DESC);
            CREATE INDEX IF NOT EXISTS idx_pending_mutations_status ON pending_remote_mutations(status, created_at ASC);
            CREATE INDEX IF NOT EXISTS idx_worm_audit_seq ON worm_audit_records(sequence ASC);

            -- Full-text search indexes are created eagerly at migration time (not lazily on
            -- first search) so no message or event saved before the first search call is ever
            -- permanently unindexed.
            --
            -- CONFIDENTIALITY TRADEOFF: `messages.body_plain` / `messages.body_html` are
            -- encrypted at rest (AES-256-GCM, see `PayloadCipher`). `messages_fts` is a
            -- standalone FTS5 table -- deliberately NOT an external-content table or trigger
            -- mirror of the `messages` columns -- because the stored body columns hold
            -- ciphertext and a trigger-based mirror would index that ciphertext verbatim
            -- (defeating search entirely). Instead `messages_fts` is populated explicitly from
            -- the plaintext body in application code, at the moment of encryption in
            -- `DatabaseEngine::save_email` (see there) and via `backfill_message_fts` below for
            -- any pre-existing rows. This means the trigram index now contains
            -- plaintext-derived body text: the FTS index itself is NOT encrypted, so message
            -- body content is recoverable from `messages_fts` by anyone with filesystem access
            -- to the SQLite database, even though the `messages.body_plain` column remains
            -- ciphertext. Column-level body encryption therefore provides only limited
            -- confidentiality while search is enabled -- full body confidentiality (an
            -- encrypted search index, or whole-database encryption) is future work and is NOT
            -- provided today.
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                id UNINDEXED,
                subject,
                sender,
                body_plain,
                tokenize = 'trigram'
            );

            -- Subject/sender are never encrypted in `messages`, so a delete-only trigger is
            -- sufficient here: it just keeps the FTS index free of orphaned rows. Insertion and
            -- update of `messages_fts` content happens explicitly in `save_email`, never via an
            -- AFTER INSERT/UPDATE trigger, because such a trigger would only ever see the
            -- ciphertext body column.
            CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                DELETE FROM messages_fts WHERE id = old.id;
            END;

            -- Calendar event summary/location are never encrypted at rest, so trigger-based
            -- mirroring (unlike the message body) introduces no confidentiality regression.
            CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
                id UNINDEXED,
                summary,
                location,
                tokenize = 'trigram'
            );

            CREATE TRIGGER IF NOT EXISTS events_ai AFTER INSERT ON calendar_events BEGIN
                INSERT INTO events_fts(id, summary, location)
                VALUES (new.id, new.summary, COALESCE(new.location, ''));
            END;

            CREATE TRIGGER IF NOT EXISTS events_ad AFTER DELETE ON calendar_events BEGIN
                DELETE FROM events_fts WHERE id = old.id;
            END;

            CREATE TRIGGER IF NOT EXISTS events_au AFTER UPDATE ON calendar_events BEGIN
                DELETE FROM events_fts WHERE id = old.id;
                INSERT INTO events_fts(id, summary, location)
                VALUES (new.id, new.summary, COALESCE(new.location, ''));
            END;
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        self.ensure_accounts_smtp_columns().await?;
        self.backfill_message_fts().await?;

        Ok(())
    }

    /// Additive, backfill-safe migration for backlog story #168: adds the
    /// `smtp_host` / `smtp_port` columns to a pre-existing `accounts` table
    /// that predates the SMTP account-endpoint feature.
    ///
    /// A fresh database already gets these columns from `CREATE TABLE IF NOT
    /// EXISTS accounts` above, so on a fresh database this is a no-op (the
    /// `PRAGMA table_info` check below finds both columns already present).
    /// For a pre-existing database file, `CREATE TABLE IF NOT EXISTS` is
    /// itself a no-op (the table already exists), so this is what actually
    /// adds the new columns -- as `NULL`-able columns, so every existing row
    /// keeps loading successfully (see [`Self::list_accounts`], which falls
    /// back to `server_host`/`server_port` for any row where `smtp_host` /
    /// `smtp_port` is still `NULL`). SQLite has no `ADD COLUMN IF NOT
    /// EXISTS`, so column presence is checked explicitly via `PRAGMA
    /// table_info` first, making this safe to run on every daemon startup.
    async fn ensure_accounts_smtp_columns(&self) -> Result<(), DatabaseError> {
        let existing_columns: Vec<String> = sqlx::query("PRAGMA table_info(accounts)")
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?
            .iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();

        if !existing_columns.iter().any(|c| c == "smtp_host") {
            sqlx::query("ALTER TABLE accounts ADD COLUMN smtp_host TEXT")
                .execute(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;
        }
        if !existing_columns.iter().any(|c| c == "smtp_port") {
            sqlx::query("ALTER TABLE accounts ADD COLUMN smtp_port INTEGER")
                .execute(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;
        }

        Ok(())
    }

    /// Backfill `messages_fts` for any `messages` row that does not yet have a matching FTS
    /// entry -- e.g. rows written before the FTS5 index existed, or written directly by a
    /// process that bypassed `save_email`'s explicit index population. Run automatically as
    /// part of [`DatabaseEngine::migrate`] (idempotent: a fully-indexed database performs no
    /// work). The stored `body_plain` column holds AES-256-GCM ciphertext, so each candidate
    /// row is decrypted with this engine's storage key before being written into the
    /// plaintext-derived trigram index -- see the confidentiality tradeoff documented above
    /// `messages_fts`'s `CREATE VIRTUAL TABLE` statement in [`DatabaseEngine::migrate`].
    async fn backfill_message_fts(&self) -> Result<(), DatabaseError> {
        let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT m.id, m.subject, m.sender, m.body_plain
            FROM messages m
            WHERE NOT EXISTS (SELECT 1 FROM messages_fts f WHERE f.id = m.id)
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        for (id, subject, sender, body_plain) in rows {
            let dec_plain = body_plain
                .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p))
                .unwrap_or_default();

            sqlx::query(
                "INSERT INTO messages_fts (id, subject, sender, body_plain) VALUES (?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(&subject)
            .bind(&sender)
            .bind(&dec_plain)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        }

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
            (id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs, smtp_host, smtp_port)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(&config.smtp_host)
        .bind(config.smtp_port as i64)
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
            Option<String>,
            Option<i64>,
        )> = sqlx::query_as(
            r#"
            SELECT id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs, smtp_host, smtp_port
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
                    smtp_host,
                    smtp_port,
                )| {
                    let protocol = serde_json::from_str(&protocol_str)
                        .unwrap_or(nuncio_core::AccountProtocol::ImapSmtp);
                    // Backfill-safe fallback (backlog story #168): a row
                    // written before `smtp_host`/`smtp_port` existed has
                    // `NULL` in both columns (see
                    // `Self::ensure_accounts_smtp_columns`). Rather than
                    // surface an incomplete/invalid config, fall back to the
                    // same host/port already used for IMAP/JMAP -- the best
                    // available default for an account configured before
                    // outbound mail had its own endpoint.
                    let resolved_smtp_host = smtp_host.unwrap_or_else(|| server_host.clone());
                    let resolved_smtp_port =
                        smtp_port.map(|p| p as u16).unwrap_or(server_port as u16);
                    nuncio_core::AccountConfig {
                        id,
                        name,
                        email_address,
                        protocol,
                        server_host,
                        server_port: server_port as u16,
                        smtp_host: resolved_smtp_host,
                        smtp_port: resolved_smtp_port,
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
    ///
    /// The message body is encrypted (AES-256-GCM) before being written to the `messages`
    /// table, but the *plaintext* body is also indexed into the standalone `messages_fts`
    /// FTS5 table at this same write, before the plaintext is discarded. This is what makes
    /// body search functional at all: a trigger mirroring the encrypted column would only ever
    /// index ciphertext. See the confidentiality tradeoff documented above the `messages_fts`
    /// `CREATE VIRTUAL TABLE` statement in [`DatabaseEngine::migrate`] -- the FTS index itself
    /// is not encrypted, so this intentionally trades some body confidentiality for working
    /// search.
    pub async fn save_email(&self, email: &nuncio_core::model::Email) -> Result<(), DatabaseError> {
        let enc_plain = email
            .body_plain
            .as_ref()
            .map(|p| crate::cipher::PayloadCipher::encrypt_text_at_rest(&self.storage_key, p));
        let enc_html = email
            .body_html
            .as_ref()
            .map(|h| crate::cipher::PayloadCipher::encrypt_text_at_rest(&self.storage_key, h));

        let mut tx = self.pool.begin().await.map_err(DatabaseError::Query)?;

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
        .execute(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?;

        // Re-indexing: delete any prior FTS row for this message id, then insert the fresh
        // plaintext-derived row. FTS5 has no natural "INSERT OR REPLACE" semantics for a
        // standalone (non-external-content) table, so this is done explicitly rather than via
        // trigger.
        sqlx::query("DELETE FROM messages_fts WHERE id = ?")
            .bind(&email.id)
            .execute(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;

        sqlx::query(
            "INSERT INTO messages_fts (id, subject, sender, body_plain) VALUES (?, ?, ?, ?)",
        )
        .bind(&email.id)
        .bind(&email.subject)
        .bind(&email.sender)
        .bind(email.body_plain.as_deref().unwrap_or(""))
        .execute(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?;

        tx.commit().await.map_err(DatabaseError::Query)?;

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
                    let dec_plain = body_plain.map(|p| {
                        crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p)
                    });
                    let dec_html = body_html.map(|h| {
                        crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h)
                    });
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
            .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p));
        let dec_html = row
            .9
            .map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h));

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

    /// Update a single message's read/unread flag in place (backlog story
    /// 1.C.4, GH #159).
    ///
    /// Returns `DatabaseError::Query(sqlx::Error::RowNotFound)` if no message
    /// with `message_id` exists, so callers can distinguish "flag flipped"
    /// from "message never existed" rather than silently succeeding on a
    /// no-op update.
    pub async fn set_message_read(
        &self,
        message_id: &str,
        read: bool,
    ) -> Result<(), DatabaseError> {
        let result = sqlx::query("UPDATE messages SET read_flag = ? WHERE id = ?")
            .bind(if read { 1i64 } else { 0i64 })
            .bind(message_id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        if result.rows_affected() == 0 {
            return Err(DatabaseError::Query(sqlx::Error::RowNotFound));
        }
        Ok(())
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
            if let Ok(mut parsed) =
                nuncio_filter::NsqlParser::parse_rule(&name, priority as i32, &nsql_text)
            {
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
        let rows: Vec<PendingMutationRow> = sqlx::query_as(
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
            .map(
                |(
                    id,
                    rule_id,
                    message_id,
                    mutation_type,
                    payload,
                    status,
                    retry_count,
                    created_at,
                )| {
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
                },
            )
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

    /// Record a cryptographically hash-chained [`nuncio_filter::FilterExecutionLog`], signed
    /// with the ledger HMAC key provisioned for this engine from the secret vault.
    pub async fn save_filter_execution_log(
        &self,
        rule_id: &str,
        message_id: &str,
        action_taken: &str,
    ) -> Result<nuncio_filter::FilterExecutionLog, DatabaseError> {
        let latest_hash: Option<(String,)> =
            sqlx::query_as("SELECT hash FROM filter_execution_logs ORDER BY id DESC LIMIT 1")
                .fetch_optional(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;

        let prev_hash = latest_hash
            .map(|r| r.0)
            .unwrap_or_else(|| "GENESIS".to_string());
        let matched_at = chrono::Utc::now().timestamp();
        let hash = compute_log_hash(
            &prev_hash,
            rule_id,
            message_id,
            action_taken,
            matched_at,
            &self.ledger_key,
        );

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
            .map(
                |(id, rule_id, message_id, action_taken, matched_at, prev_hash, hash)| {
                    nuncio_filter::FilterExecutionLog {
                        id,
                        rule_id,
                        message_id,
                        action_taken,
                        matched_at,
                        prev_hash,
                        hash,
                    }
                },
            )
            .collect())
    }

    /// Verify cryptographic hash-chain ledger integrity for filter execution logs, using
    /// the ledger HMAC key provisioned for this engine.
    pub async fn verify_execution_log_chain(&self) -> Result<bool, DatabaseError> {
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
            let computed = compute_log_hash(
                &prev_hash,
                &rule_id,
                &message_id,
                &action_taken,
                matched_at,
                &self.ledger_key,
            );
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

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let dec_plain = r.8.map(|p| {
                    crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p)
                });
                let dec_html = r.9.map(|h| {
                    crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h)
                });
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

    /// Query messages across the WHOLE store for export purposes (backlog
    /// story 2.B, GH #172), optionally narrowed to a single account or a
    /// single folder. Passing `None` for both returns every message in the
    /// store. Ordered by `id` ascending, mirroring [`Self::get_message_chunk`]'s
    /// deterministic keyset ordering.
    #[allow(clippy::type_complexity)]
    pub async fn list_messages_for_export(
        &self,
        account_id: Option<&str>,
        folder_id: Option<&str>,
    ) -> Result<Vec<nuncio_core::model::Email>, DatabaseError> {
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT id, account_id, folder_id, subject, sender, recipient, received_at, read_flag, body_plain, body_html FROM messages"
        );

        let mut has_filter = false;
        if let Some(account_id) = account_id {
            builder.push(" WHERE account_id = ");
            builder.push_bind(account_id);
            has_filter = true;
        }
        if let Some(folder_id) = folder_id {
            builder.push(if has_filter {
                " AND folder_id = "
            } else {
                " WHERE folder_id = "
            });
            builder.push_bind(folder_id);
        }
        builder.push(" ORDER BY id ASC");

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

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let dec_plain = r.8.map(|p| {
                    crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p)
                });
                let dec_html = r.9.map(|h| {
                    crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h)
                });
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

    /// Append a new immutable WORM audit record to the log ledger, signed with the WORM
    /// HMAC key provisioned for this engine from the secret vault.
    pub async fn append_worm_audit_record(
        &self,
        actor: &str,
        action: &str,
        data_payload: &[u8],
    ) -> Result<nuncio_core::WormAuditRecord, DatabaseError> {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as i64;

        let last_row: Option<(i64, String)> = sqlx::query_as(
            "SELECT sequence, record_hmac FROM worm_audit_records ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        let (next_seq, prev_hash) = match last_row {
            Some((seq, hash)) => (seq as u64 + 1, hash),
            None => (1, "GENESIS".to_string()),
        };

        let record = nuncio_core::WormAuditRecord::create_signed(
            &self.worm_key,
            next_seq,
            now_ns,
            actor,
            action,
            data_payload,
            &prev_hash,
        )
        .map_err(|e| DatabaseError::ChainIntegrityFailed(e.to_string()))?;

        sqlx::query(
            "INSERT INTO worm_audit_records (sequence, timestamp_ns, actor, action, data_hash, previous_block_hash, record_hmac)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.sequence as i64)
        .bind(record.timestamp_ns)
        .bind(&record.actor)
        .bind(&record.action)
        .bind(&record.data_hash)
        .bind(&record.previous_block_hash)
        .bind(&record.record_hmac)
        .execute(&self.pool)
        .await?;

        Ok(record)
    }

    /// List WORM audit records in order of sequence.
    pub async fn list_worm_audit_records(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<nuncio_core::WormAuditRecord>, DatabaseError> {
        let rows: Vec<(i64, i64, String, String, String, String, String)> = sqlx::query_as(
            "SELECT sequence, timestamp_ns, actor, action, data_hash, previous_block_hash, record_hmac
             FROM worm_audit_records ORDER BY sequence ASC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| nuncio_core::WormAuditRecord {
                sequence: r.0 as u64,
                timestamp_ns: r.1,
                actor: r.2,
                action: r.3,
                data_hash: r.4,
                previous_block_hash: r.5,
                record_hmac: r.6,
            })
            .collect())
    }

    /// Verify the entire WORM cryptographic audit log chain, using the WORM HMAC key
    /// provisioned for this engine.
    pub async fn verify_worm_audit_chain(&self) -> Result<(), DatabaseError> {
        let records = self.list_worm_audit_records(100_000, 0).await?;
        nuncio_core::verify_worm_chain(&records, &self.worm_key)
            .map_err(|e| DatabaseError::ChainIntegrityFailed(e.to_string()))
    }

    /// Re-verify the entire WORM audit ledger and report a structured
    /// result (backlog story 2.B, GH #172), rather than the opaque
    /// pass/fail `Result` [`Self::verify_worm_audit_chain`] returns -- so
    /// callers (e.g. the `Audit/VerifyChain` gRPC RPC) can honestly report
    /// exactly how many records were checked and, if the chain is broken,
    /// exactly which sequence broke it. `first_broken_seq` is populated ONLY
    /// when the underlying failure is a genuine
    /// `WormAuditError::TamperingDetected` (a specific record's HMAC or
    /// chain linkage did not verify) -- never guessed. Any other failure
    /// (e.g. a signing error) fails closed as `Err`, never reported as a
    /// fabricated `valid: false`.
    pub async fn verify_worm_audit_chain_report(&self) -> Result<WormChainReport, DatabaseError> {
        let records = self.list_worm_audit_records(100_000, 0).await?;
        let record_count = records.len();
        match nuncio_core::verify_worm_chain(&records, &self.worm_key) {
            Ok(()) => Ok(WormChainReport {
                valid: true,
                record_count,
                first_broken_seq: None,
            }),
            Err(nuncio_core::WormAuditError::TamperingDetected { sequence, .. }) => {
                Ok(WormChainReport {
                    valid: false,
                    record_count,
                    first_broken_seq: Some(sequence),
                })
            }
            Err(e) => Err(DatabaseError::ChainIntegrityFailed(e.to_string())),
        }
    }

    /// Export a collection of email messages to a target output path in the requested portable format.
    pub async fn export_messages_to_file(
        &self,
        messages: &[nuncio_core::model::Email],
        format: nuncio_core::ExportFormat,
        output_path: &Path,
    ) -> Result<nuncio_core::ExportSummary, DatabaseError> {
        let file = std::fs::File::create(output_path).map_err(|e| {
            DatabaseError::RecoveryFailed(format!("failed to create output file: {e}"))
        })?;

        let bytes_written = match format {
            nuncio_core::ExportFormat::Mbox => {
                nuncio_core::ExportEngine::export_mbox(messages, file)
                    .map_err(|e| DatabaseError::RecoveryFailed(e.to_string()))?
            }
            nuncio_core::ExportFormat::EmlZip => {
                nuncio_core::ExportEngine::export_eml_zip(messages, file)
                    .map_err(|e| DatabaseError::RecoveryFailed(e.to_string()))?
            }
            nuncio_core::ExportFormat::Json => {
                nuncio_core::ExportEngine::export_json(messages, file)
                    .map_err(|e| DatabaseError::RecoveryFailed(e.to_string()))?
            }
            nuncio_core::ExportFormat::JsonLines => {
                nuncio_core::ExportEngine::export_jsonl(messages, file)
                    .map_err(|e| DatabaseError::RecoveryFailed(e.to_string()))?
            }
        };

        // Record WORM audit log for this data export
        let _ = self
            .append_worm_audit_record(
                "system.export",
                "data.export",
                output_path.to_string_lossy().as_bytes(),
            )
            .await;

        Ok(nuncio_core::ExportSummary {
            output_path: output_path.to_string_lossy().to_string(),
            format,
            message_count: messages.len(),
            bytes_written,
        })
    }
}

fn compute_log_hash(
    prev_hash: &str,
    rule_id: &str,
    message_id: &str,
    action_taken: &str,
    matched_at: i64,
    secret_key: &[u8],
) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    // HMAC accepts keys of any length (RFC 2104), so this never fails in practice.
    let Ok(mut mac) = HmacSha256::new_from_slice(secret_key) else {
        return String::new();
    };
    let payload = format!("{prev_hash}:{rule_id}:{message_id}:{action_taken}:{matched_at}");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    /// Proves the fix for 1.B.4: a transient error surfacing from the integrity probe (here, a
    /// genuine pool-exhaustion / acquire-timeout condition, deterministically forced by holding
    /// the pool's only connection) must propagate as `Err`, never be coerced into `Ok(false)`
    /// (which would be indistinguishable from genuine corruption to `open_with_backup_dir` and
    /// would trigger destructive salvage of a perfectly healthy database).
    #[tokio::test]
    async fn check_integrity_pool_propagates_transient_timeout_without_masking_as_corrupt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("busy_test.db");
        let secrets = crate::vault::SecretManager::mock();
        {
            let engine = DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .unwrap();
            engine.close().await;
        }

        let url = format!("sqlite://{}", db_path.to_string_lossy());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(200))
            .connect(&url)
            .await
            .unwrap();

        // Hold the pool's only connection so a concurrent acquire attempt cannot succeed and
        // must time out -- a genuine, deterministic transient condition (no SQLite-level lock
        // contention required).
        let _held = pool.acquire().await.unwrap();

        let result = DatabaseEngine::check_integrity_pool(&pool).await;
        assert!(
            matches!(result, Err(DatabaseError::Query(sqlx::Error::PoolTimedOut))),
            "a pool-exhaustion / acquire-timeout condition must propagate as an Err, never be \
             silently coerced into Ok(false); got: {result:?}"
        );
    }

    /// End-to-end proof of 1.B.4: a genuinely transient, operational condition (another
    /// connection holding an exclusive lock on the live database) must never be treated as
    /// corruption by `open_with_backup_dir` -- no backup isolation, no salvage, no deletion of
    /// the live file. The database and its data must survive completely intact, and a retry
    /// once the contention clears must succeed with no recovery recorded.
    #[tokio::test]
    async fn open_with_backup_dir_never_salvages_on_transient_lock_contention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("contended.db");
        let backup_dir = dir.path().join("backups");
        let secrets = crate::vault::SecretManager::mock();

        // Create a valid, healthy database with real data.
        {
            let engine = DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .unwrap();
            let acct = nuncio_core::AccountConfig {
                id: "acct-contended-1".to_string(),
                name: "Contended Account".to_string(),
                email_address: "contended@nuncio.mx".to_string(),
                protocol: nuncio_core::AccountProtocol::ImapSmtp,
                server_host: "imap.nuncio.mx".to_string(),
                server_port: 993,
                smtp_host: "smtp.nuncio.mx".to_string(),
                smtp_port: 465,
                use_tls: true,
                imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                keyring_secret_key: "nuncio/acct-contended-1".to_string(),
                sync_interval_secs: 60,
            };
            engine.save_account(&acct).await.unwrap();
            engine.close().await;
        }

        // Hold an exclusive lock on the file from an independent connection, forcing any other
        // connection attempt against the same file into a genuine SQLITE_BUSY -- a transient,
        // operational condition, not corruption.
        let url = format!("sqlite://{}", db_path.to_string_lossy());
        let mut locker = sqlx::sqlite::SqliteConnection::connect(&url)
            .await
            .expect("locker connects");
        sqlx::query("PRAGMA locking_mode=EXCLUSIVE;")
            .execute(&mut locker)
            .await
            .expect("set exclusive locking mode");
        sqlx::query("BEGIN IMMEDIATE;")
            .execute(&mut locker)
            .await
            .expect("begin immediate");
        // Locking mode only takes effect on the next read/write, so force one now.
        sqlx::query("SELECT COUNT(*) FROM accounts;")
            .execute(&mut locker)
            .await
            .expect("force exclusive lock acquisition");

        let result = DatabaseEngine::open_with_backup_dir(&db_path, &backup_dir, &secrets).await;
        assert!(
            result.is_err(),
            "a locked/busy database must surface an error, not silently succeed: {result:?}"
        );
        let backup_is_empty =
            !backup_dir.exists() || std::fs::read_dir(&backup_dir).unwrap().next().is_none();
        assert!(
            backup_is_empty,
            "a transient lock/busy condition must NEVER trigger backup isolation + salvage"
        );

        // Release the lock deterministically before checking that the live database survived.
        let _ = sqlx::query("ROLLBACK;").execute(&mut locker).await;
        let _ = locker.close().await;

        // The live database and its data must be completely intact, and a retry succeeds with
        // no recovery recorded.
        let (engine, summary) =
            DatabaseEngine::open_with_backup_dir(&db_path, &backup_dir, &secrets)
                .await
                .expect("reopen succeeds once contention clears");
        assert!(
            summary.is_none(),
            "no recovery should have been recorded for a transient condition"
        );
        let accounts = engine.list_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acct-contended-1");
    }

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
    async fn connect_file_creates_missing_parent_directory() {
        // Regression (found via dogfooding): a fresh install opens the DB at a
        // path whose parent directory (e.g. ~/.nuncio) does not exist yet.
        // `create_if_missing` creates the file, not the dir, so without an
        // explicit create_dir_all the daemon failed to start with CANTOPEN.
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("nested").join("data").join("nuncio.db");
        assert!(!db_path.parent().expect("has parent").exists());

        let secrets = crate::vault::SecretManager::mock();
        let engine = DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .expect("connect_file creates the missing parent dir and opens");

        assert!(db_path.parent().expect("has parent").exists());
        assert!(engine.check_integrity().await.expect("integrity check"));
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

    /// Backlog story 1.C.4 (GH #159): `set_message_read` flips the persisted
    /// `read_flag` in place (both directions), and reports
    /// `sqlx::Error::RowNotFound` for a message ID that was never saved,
    /// rather than silently succeeding on a no-op update.
    #[tokio::test]
    async fn set_message_read_flips_flag_and_reports_not_found_for_missing_message() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let email = nuncio_core::model::Email {
            id: "msg-mark-1".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "INBOX".to_string(),
            subject: "Mark Read Test".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: false,
            body_plain: None,
            body_html: None,
            attachments: Vec::new(),
        };
        engine.save_email(&email).await.expect("save email");

        engine
            .set_message_read("msg-mark-1", true)
            .await
            .expect("mark read succeeds");
        let fetched = engine
            .get_message("msg-mark-1")
            .await
            .expect("get message succeeds");
        assert!(fetched.read);

        engine
            .set_message_read("msg-mark-1", false)
            .await
            .expect("mark unread succeeds");
        let fetched = engine
            .get_message("msg-mark-1")
            .await
            .expect("get message succeeds");
        assert!(!fetched.read);

        let err = engine
            .set_message_read("msg-does-not-exist", true)
            .await
            .expect_err("marking an unknown message must fail");
        assert!(matches!(
            err,
            DatabaseError::Query(sqlx::Error::RowNotFound)
        ));
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
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
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
        assert_eq!(accounts[0].smtp_host, "smtp.nuncio.mx");
        assert_eq!(accounts[0].smtp_port, 465);
    }

    /// Backlog story #168: proves the additive `smtp_host`/`smtp_port`
    /// migration is backfill-safe. Simulates a database created BEFORE this
    /// feature existed (an `accounts` table with no `smtp_host`/`smtp_port`
    /// columns at all, populated via a direct `INSERT` bypassing
    /// `save_account`), then opens it through `DatabaseEngine::connect_file`
    /// (which runs `migrate()`, including `ensure_accounts_smtp_columns`)
    /// and confirms: (1) opening an old-schema database never errors, (2)
    /// the pre-existing row still loads, falling back to `server_host`/
    /// `server_port` for the missing SMTP endpoint, and (3) re-running
    /// `migrate()` a second time (simulating a daemon restart) is a no-op
    /// that does not error or duplicate columns.
    #[tokio::test]
    async fn migrate_backfills_smtp_columns_for_a_pre_existing_accounts_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("pre_smtp_migration.db");
        let secrets = crate::vault::SecretManager::mock();

        // Step 1: create an OLD-schema `accounts` table (no smtp_host/smtp_port
        // columns) directly, bypassing `DatabaseEngine::migrate` entirely, and
        // insert one pre-existing row -- exactly what a database created
        // before backlog story #168 would look like on disk.
        {
            let url = format!("sqlite://{}", db_path.to_string_lossy());
            let options = sqlx::sqlite::SqliteConnectOptions::from_str(&url)
                .expect("valid sqlite url")
                .create_if_missing(true);
            let pool = sqlx::SqlitePool::connect_with(options)
                .await
                .expect("connect to fresh old-schema db");

            sqlx::query(
                r#"
                CREATE TABLE accounts (
                    id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL,
                    email_address TEXT NOT NULL,
                    protocol TEXT NOT NULL,
                    server_host TEXT NOT NULL,
                    server_port INTEGER NOT NULL,
                    use_tls INTEGER NOT NULL,
                    keyring_secret_key TEXT NOT NULL,
                    sync_interval_secs INTEGER NOT NULL
                )
                "#,
            )
            .execute(&pool)
            .await
            .expect("create old-schema accounts table");

            sqlx::query(
                r#"
                INSERT INTO accounts
                (id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs)
                VALUES ('acct-pre-migration', 'Pre-Migration Account', 'pre@nuncio.mx', '"imap-smtp"', 'imap.nuncio.mx', 993, 1, 'nuncio/acct-pre-migration', 60)
                "#,
            )
            .execute(&pool)
            .await
            .expect("insert pre-existing row");

            pool.close().await;
        }

        // Step 2: open through the real production path -- this is what
        // actually runs `migrate()` / `ensure_accounts_smtp_columns`.
        let engine = DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .expect("opening an old-schema database must never error");

        let accounts = engine
            .list_accounts()
            .await
            .expect("list accounts succeeds on a migrated old-schema table");
        assert_eq!(accounts.len(), 1);
        let acct = &accounts[0];
        assert_eq!(acct.id, "acct-pre-migration");
        // Falls back to the IMAP/JMAP endpoint since smtp_host/smtp_port
        // were NULL for this pre-existing row.
        assert_eq!(acct.smtp_host, "imap.nuncio.mx");
        assert_eq!(acct.smtp_port, 993);

        // Step 3: re-running migrate() (e.g. a second daemon startup against
        // the same file) must be a no-op, not an error.
        engine
            .migrate()
            .await
            .expect("re-running migrate on an already-migrated table must not error");

        // A newly saved account (going through the real `save_account` path)
        // now gets its own genuine smtp_host/smtp_port persisted, proving
        // the migrated columns are fully writable/readable going forward.
        let new_acct = nuncio_core::AccountConfig {
            id: "acct-post-migration".to_string(),
            name: "Post Migration Account".to_string(),
            email_address: "post@nuncio.mx".to_string(),
            protocol: nuncio_core::AccountProtocol::ImapSmtp,
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 587,
            use_tls: true,
            imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            smtp_tls_mode: nuncio_core::TlsMode::StartTls,
            keyring_secret_key: "nuncio/acct-post-migration".to_string(),
            sync_interval_secs: 60,
        };
        engine
            .save_account(&new_acct)
            .await
            .expect("save account succeeds after migration");
        let accounts = engine.list_accounts().await.expect("list accounts");
        let post = accounts
            .iter()
            .find(|a| a.id == "acct-post-migration")
            .expect("newly saved account present");
        assert_eq!(post.smtp_host, "smtp.nuncio.mx");
        assert_eq!(post.smtp_port, 587);

        engine.close().await;
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

        engine
            .save_filter_rule(&rule)
            .await
            .expect("save filter rule");
        let rules = engine.list_filter_rules().await.expect("list filter rules");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "Spam Filter");

        // Hash-chained logs, signed with the ledger key this ephemeral engine provisioned
        // for itself from `MockKeyring` (no hardcoded secret involved).
        let log1 = engine
            .save_filter_execution_log(&rule.id, "msg-100", "DELETE")
            .await
            .unwrap();
        let log2 = engine
            .save_filter_execution_log(&rule.id, "msg-101", "DELETE")
            .await
            .unwrap();

        assert_eq!(log1.prev_hash, "GENESIS");
        assert_eq!(log2.prev_hash, log1.hash);

        let is_valid = engine.verify_execution_log_chain().await.unwrap();
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

        let mutation = nuncio_filter::OutboxManager::create_mutation(
            "rule-1",
            "msg-001",
            "MOVE",
            Some("Archive".to_string()),
        );
        engine.save_pending_mutation(&mutation).await.unwrap();

        let pending = engine.list_pending_mutations(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, mutation.id);

        engine
            .update_mutation_status(&mutation.id, "completed", 1)
            .await
            .unwrap();
        let pending_after = engine.list_pending_mutations(10).await.unwrap();
        assert_eq!(pending_after.len(), 0);
    }

    /// Proves that `DatabaseEngine` sources its WORM HMAC key from the injected
    /// `SecretManager` vault rather than any compiled-in default: an engine backed by one
    /// (mock) vault can sign and verify its own WORM chain, but a second engine backed by an
    /// *independent* mock vault -- and therefore holding different randomly-generated key
    /// material -- must fail to verify records signed under the first vault's key.
    #[tokio::test]
    async fn worm_audit_key_is_vault_sourced_not_a_shared_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("worm_vault_isolation_test.sqlite");

        let secrets_a = crate::vault::SecretManager::mock();
        let engine_a = DatabaseEngine::connect_file(&db_path, &secrets_a)
            .await
            .expect("engine_a connects");
        engine_a
            .append_worm_audit_record("system.test", "test.action", b"payload")
            .await
            .expect("record signed with engine_a's vault key");
        assert!(engine_a.verify_worm_audit_chain().await.is_ok());
        engine_a.close().await;

        let secrets_b = crate::vault::SecretManager::mock();
        let engine_b = DatabaseEngine::connect_file(&db_path, &secrets_b)
            .await
            .expect("engine_b connects to the same file with an independent vault");
        let result = engine_b.verify_worm_audit_chain().await;
        assert!(
            result.is_err(),
            "an independently-vaulted engine must not validate another vault's WORM chain \
             (would indicate a shared/hardcoded key instead of real vault-sourced material)"
        );
    }

    /// Same proof as above, for the filter execution log ledger HMAC key.
    #[tokio::test]
    async fn ledger_key_is_vault_sourced_not_a_shared_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("ledger_vault_isolation_test.sqlite");

        let secrets_a = crate::vault::SecretManager::mock();
        let engine_a = DatabaseEngine::connect_file(&db_path, &secrets_a)
            .await
            .expect("engine_a connects");
        engine_a
            .save_filter_execution_log("rule-1", "msg-1", "DELETE")
            .await
            .expect("log signed with engine_a's vault key");
        assert!(engine_a.verify_execution_log_chain().await.unwrap());
        engine_a.close().await;

        let secrets_b = crate::vault::SecretManager::mock();
        let engine_b = DatabaseEngine::connect_file(&db_path, &secrets_b)
            .await
            .expect("engine_b connects to the same file with an independent vault");
        let is_valid = engine_b
            .verify_execution_log_chain()
            .await
            .expect("verification query succeeds");
        assert!(
            !is_valid,
            "an independently-vaulted engine must not validate another vault's ledger chain"
        );
    }

    fn export_test_email(id: &str, account_id: &str, folder_id: &str) -> nuncio_core::model::Email {
        nuncio_core::model::Email {
            id: id.to_string(),
            account_id: account_id.to_string(),
            folder_id: folder_id.to_string(),
            subject: format!("Subject {id}"),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1_700_000_000,
            read: false,
            body_plain: Some(format!("Body {id}")),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn list_messages_for_export_with_no_filters_returns_every_message() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        engine
            .save_email(&export_test_email("msg-1", "acct-a", "inbox"))
            .await
            .unwrap();
        engine
            .save_email(&export_test_email("msg-2", "acct-b", "archive"))
            .await
            .unwrap();

        let all = engine.list_messages_for_export(None, None).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "msg-1");
        assert_eq!(all[1].id, "msg-2");
    }

    #[tokio::test]
    async fn list_messages_for_export_filters_by_account_id() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        engine
            .save_email(&export_test_email("msg-1", "acct-a", "inbox"))
            .await
            .unwrap();
        engine
            .save_email(&export_test_email("msg-2", "acct-b", "inbox"))
            .await
            .unwrap();

        let scoped = engine
            .list_messages_for_export(Some("acct-a"), None)
            .await
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "msg-1");
    }

    #[tokio::test]
    async fn list_messages_for_export_filters_by_folder_id() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        engine
            .save_email(&export_test_email("msg-1", "acct-a", "inbox"))
            .await
            .unwrap();
        engine
            .save_email(&export_test_email("msg-2", "acct-a", "archive"))
            .await
            .unwrap();

        let scoped = engine
            .list_messages_for_export(None, Some("archive"))
            .await
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "msg-2");
    }

    #[tokio::test]
    async fn list_messages_for_export_filters_by_account_and_folder_together() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        engine
            .save_email(&export_test_email("msg-1", "acct-a", "inbox"))
            .await
            .unwrap();
        engine
            .save_email(&export_test_email("msg-2", "acct-a", "archive"))
            .await
            .unwrap();
        engine
            .save_email(&export_test_email("msg-3", "acct-b", "inbox"))
            .await
            .unwrap();

        let scoped = engine
            .list_messages_for_export(Some("acct-a"), Some("inbox"))
            .await
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "msg-1");
    }

    #[tokio::test]
    async fn verify_worm_audit_chain_report_reports_valid_on_an_intact_chain() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        engine
            .append_worm_audit_record("system.test", "action.one", b"payload-1")
            .await
            .unwrap();
        engine
            .append_worm_audit_record("system.test", "action.two", b"payload-2")
            .await
            .unwrap();

        let report = engine.verify_worm_audit_chain_report().await.unwrap();
        assert!(report.valid);
        assert_eq!(report.record_count, 2);
        assert_eq!(report.first_broken_seq, None);
    }

    #[tokio::test]
    async fn verify_worm_audit_chain_report_reports_empty_chain_as_valid() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let report = engine.verify_worm_audit_chain_report().await.unwrap();
        assert!(report.valid);
        assert_eq!(report.record_count, 0);
        assert_eq!(report.first_broken_seq, None);
    }

    #[tokio::test]
    async fn verify_worm_audit_chain_report_reports_first_broken_seq_on_a_vault_key_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("worm_report_mismatch_test.sqlite");

        let secrets_a = crate::vault::SecretManager::mock();
        let engine_a = DatabaseEngine::connect_file(&db_path, &secrets_a)
            .await
            .expect("engine_a connects");
        engine_a
            .append_worm_audit_record("system.test", "action.one", b"payload-1")
            .await
            .expect("record signed with engine_a's vault key");
        engine_a.close().await;

        let secrets_b = crate::vault::SecretManager::mock();
        let engine_b = DatabaseEngine::connect_file(&db_path, &secrets_b)
            .await
            .expect("engine_b connects to the same file with an independent vault");

        let report = engine_b
            .verify_worm_audit_chain_report()
            .await
            .expect("verification query succeeds (an invalid chain is a normal result)");
        assert!(!report.valid);
        assert_eq!(report.record_count, 1);
        assert_eq!(report.first_broken_seq, Some(1));
    }
}
