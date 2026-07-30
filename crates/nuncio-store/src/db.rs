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
    /// A body payload failed AEAD encryption before it could be persisted at rest.
    /// This is a fail-closed error: it is returned instead of ever writing an
    /// empty/placeholder ciphertext that would be indistinguishable from a
    /// genuinely empty body on read-back.
    #[error("payload encryption failed: {0}")]
    Encryption(String),
}

impl From<crate::cipher::CipherError> for DatabaseError {
    fn from(err: crate::cipher::CipherError) -> Self {
        DatabaseError::Encryption(err.to_string())
    }
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

    /// Returns true if this error means "no row matched the lookup" (e.g. a
    /// `get_message`/`get_contact`/`get_calendar_event` call for an id that
    /// was never saved), as opposed to a genuine I/O, corruption, or key
    /// provisioning failure.
    ///
    /// Exposed so callers outside this crate can distinguish "not found" from
    /// other failures without depending on `sqlx` directly.
    pub fn is_not_found(&self) -> bool {
        matches!(self, DatabaseError::Query(sqlx::Error::RowNotFound))
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

/// Reconstruct a [`nuncio_contacts::Contact`] from a `contacts` table row.
///
/// `created_at`/`updated_at`/`last_interacted_at` are stored as RFC 3339 strings written by
/// [`DatabaseEngine::save_contact`] itself, so a parse failure here would indicate corrupted
/// storage rather than bad input; falling back to "now" for that edge case (rather than
/// propagating a parse error through every read) mirrors the tolerant `unwrap_or_default`
/// JSON-decode style already used for `emails_json`/`phones_json` below.
fn contact_from_row(row: &sqlx::sqlite::SqliteRow) -> nuncio_contacts::Contact {
    let emails_json: String = row.get("emails_json");
    let phones_json: String = row.get("phones_json");
    let emails: Vec<nuncio_contacts::ContactEmail> =
        serde_json::from_str(&emails_json).unwrap_or_default();
    let phones: Vec<nuncio_contacts::ContactPhone> =
        serde_json::from_str(&phones_json).unwrap_or_default();
    let is_favorite: i64 = row.get("is_favorite");
    let interaction_count: i64 = row.get("interaction_count");
    let last_interacted_at: Option<String> = row.get("last_interacted_at");
    let created_at: String = row.get("created_at");
    let updated_at: String = row.get("updated_at");

    let parse_rfc3339 = |s: &str| -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now())
    };

    nuncio_contacts::Contact {
        id: row.get("id"),
        account_id: row.get("account_id"),
        display_name: row.get("display_name"),
        given_name: row.get("given_name"),
        family_name: row.get("family_name"),
        organization: row.get("organization"),
        job_title: row.get("job_title"),
        notes: row.get("notes"),
        avatar_url: row.get("avatar_url"),
        emails,
        phones,
        is_favorite: is_favorite != 0,
        interaction_count: interaction_count as u64,
        last_interacted_at: last_interacted_at.as_deref().map(parse_rfc3339),
        created_at: parse_rfc3339(&created_at),
        updated_at: parse_rfc3339(&updated_at),
    }
}

/// Structured result of re-verifying the entire persisted WORM audit
/// ledger's hash chain. See
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

/// Serialize a [`nuncio_core::TlsMode`] to its stable on-disk `accounts`
/// column form. Kept as a bare snake_case token (matching the column's
/// `'implicit_tls'` default) rather than JSON, so a row inserted by the
/// migration default and one written here are byte-identical.
fn tls_mode_to_db(mode: nuncio_core::TlsMode) -> &'static str {
    match mode {
        nuncio_core::TlsMode::ImplicitTls => "implicit_tls",
        nuncio_core::TlsMode::StartTls => "start_tls",
        nuncio_core::TlsMode::Plain => "plain",
    }
}

/// Parse a persisted `accounts` TLS-mode column back into
/// [`nuncio_core::TlsMode`]. An unrecognized value falls back to the safe
/// default (implicit TLS) rather than failing the whole account load.
fn tls_mode_from_db(value: &str) -> nuncio_core::TlsMode {
    match value {
        "start_tls" => nuncio_core::TlsMode::StartTls,
        "plain" => nuncio_core::TlsMode::Plain,
        _ => nuncio_core::TlsMode::ImplicitTls,
    }
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
            .foreign_keys(true)
            // SQLite only fires a table's AFTER DELETE trigger for the implicit delete an
            // `INSERT OR REPLACE` performs on a primary-key conflict when `recursive_triggers`
            // is enabled -- otherwise the delete half is silent. `calendar_events`'s
            // `events_ad`/`events_au` triggers (see `migrate`) rely on exactly this to keep
            // `events_fts` from accumulating a stale row every time `save_calendar_event`
            // upserts an existing event.
            .pragma("recursive_triggers", "ON");

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
                smtp_port INTEGER,
                imap_tls_mode TEXT NOT NULL DEFAULT 'implicit_tls',
                smtp_tls_mode TEXT NOT NULL DEFAULT 'implicit_tls'
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

            -- Contacts (nuncio_contacts::Contact). Emails/phones are stored as JSON columns
            -- (mirroring the schema already proven in nuncio-contacts's own, now-superseded
            -- ContactsDatabase) rather than normalized child tables, since a contact's email/
            -- phone list is always read and written as a whole with its parent record. Like
            -- calendar summary/location, none of this is encrypted at rest, so trigger-based
            -- FTS mirroring introduces no confidentiality regression.
            CREATE TABLE IF NOT EXISTS contacts (
                id TEXT PRIMARY KEY NOT NULL,
                account_id TEXT,
                display_name TEXT NOT NULL,
                given_name TEXT,
                family_name TEXT,
                organization TEXT,
                job_title TEXT,
                notes TEXT,
                avatar_url TEXT,
                emails_json TEXT NOT NULL DEFAULT '[]',
                phones_json TEXT NOT NULL DEFAULT '[]',
                is_favorite INTEGER NOT NULL DEFAULT 0,
                interaction_count INTEGER NOT NULL DEFAULT 0,
                last_interacted_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS contacts_fts USING fts5(
                id UNINDEXED,
                display_name,
                organization,
                emails_json,
                tokenize = 'trigram'
            );

            CREATE TRIGGER IF NOT EXISTS contacts_ai AFTER INSERT ON contacts BEGIN
                INSERT INTO contacts_fts(id, display_name, organization, emails_json)
                VALUES (new.id, new.display_name, COALESCE(new.organization, ''), new.emails_json);
            END;

            CREATE TRIGGER IF NOT EXISTS contacts_ad AFTER DELETE ON contacts BEGIN
                DELETE FROM contacts_fts WHERE id = old.id;
            END;

            CREATE TRIGGER IF NOT EXISTS contacts_au AFTER UPDATE ON contacts BEGIN
                DELETE FROM contacts_fts WHERE id = old.id;
                INSERT INTO contacts_fts(id, display_name, organization, emails_json)
                VALUES (new.id, new.display_name, COALESCE(new.organization, ''), new.emails_json);
            END;

            -- Per-folder inbound-sync checkpoint. `state` is an opaque,
            -- protocol-defined resumption token (an IMAP UID boundary such as
            -- UIDNEXT, a JMAP state string) that a backend hands back after a
            -- sync and expects to receive on the next sync so it can fetch only
            -- what changed since. Keyed by (account, folder) because the token
            -- is meaningful only within a single mailbox on a single account.
            CREATE TABLE IF NOT EXISTS folder_sync_state (
                account_id TEXT NOT NULL,
                folder_id TEXT NOT NULL,
                state TEXT NOT NULL,
                PRIMARY KEY (account_id, folder_id)
            );
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        self.ensure_accounts_smtp_columns().await?;
        self.ensure_accounts_tls_mode_columns().await?;
        self.backfill_message_fts().await?;

        Ok(())
    }

    /// Additive, backfill-safe migration that adds the `imap_tls_mode` /
    /// `smtp_tls_mode` columns to a pre-existing `accounts` table that
    /// predates persisted per-protocol TLS transport modes.
    ///
    /// A fresh database already gets these columns from `CREATE TABLE IF NOT
    /// EXISTS accounts` above, so on a fresh database this is a no-op. For a
    /// pre-existing database file the columns are added with a
    /// `'implicit_tls'` default, so every existing row keeps loading
    /// successfully and retains the historical behavior (implicit TLS) it was
    /// created under. SQLite has no `ADD COLUMN IF NOT EXISTS`, so column
    /// presence is checked explicitly via `PRAGMA table_info` first, making
    /// this safe to run on every daemon startup.
    async fn ensure_accounts_tls_mode_columns(&self) -> Result<(), DatabaseError> {
        let existing_columns: Vec<String> = sqlx::query("PRAGMA table_info(accounts)")
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?
            .iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();

        if !existing_columns.iter().any(|c| c == "imap_tls_mode") {
            sqlx::query(
                "ALTER TABLE accounts ADD COLUMN imap_tls_mode TEXT NOT NULL DEFAULT 'implicit_tls'",
            )
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        }
        if !existing_columns.iter().any(|c| c == "smtp_tls_mode") {
            sqlx::query(
                "ALTER TABLE accounts ADD COLUMN smtp_tls_mode TEXT NOT NULL DEFAULT 'implicit_tls'",
            )
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        }

        Ok(())
    }

    /// Additive, backfill-safe migration that adds the
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
            (id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs, smtp_host, smtp_port, imap_tls_mode, smtp_tls_mode)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(tls_mode_to_db(config.imap_tls_mode))
        .bind(tls_mode_to_db(config.smtp_tls_mode))
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
    }

    /// Fetch a single persisted [`nuncio_core::AccountConfig`] by id, or
    /// `None` if no such account row exists.
    pub async fn get_account(
        &self,
        id: &str,
    ) -> Result<Option<nuncio_core::AccountConfig>, DatabaseError> {
        Ok(self.list_accounts().await?.into_iter().find(|a| a.id == id))
    }

    /// Delete a persisted account configuration by id. Idempotent: deleting a
    /// non-existent id is not an error (the post-condition -- "no account row
    /// with this id" -- already holds).
    pub async fn delete_account(&self, id: &str) -> Result<(), DatabaseError> {
        sqlx::query("DELETE FROM accounts WHERE id = ?")
            .bind(id)
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
            String,
            String,
        )> = sqlx::query_as(
            r#"
            SELECT id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs, smtp_host, smtp_port, imap_tls_mode, smtp_tls_mode
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
                    imap_tls_mode,
                    smtp_tls_mode,
                )| {
                    let protocol = serde_json::from_str(&protocol_str)
                        .unwrap_or(nuncio_core::AccountProtocol::ImapSmtp);
                    // Backfill-safe fallback: a row
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
                        imap_tls_mode: tls_mode_from_db(&imap_tls_mode),
                        smtp_tls_mode: tls_mode_from_db(&smtp_tls_mode),
                        keyring_secret_key,
                        sync_interval_secs: sync_interval_secs as u64,
                    }
                },
            )
            .collect())
    }

    /// Fetch the persisted inbound-sync checkpoint for a folder, or `None` if
    /// this account/folder pair has never completed a sync. A `None` result is
    /// the signal to the backend that it must perform a full first-time fetch.
    pub async fn get_folder_sync_state(
        &self,
        account_id: &str,
        folder_id: &str,
    ) -> Result<Option<String>, DatabaseError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT state FROM folder_sync_state WHERE account_id = ? AND folder_id = ?",
        )
        .bind(account_id)
        .bind(folder_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;
        Ok(row.map(|(state,)| state))
    }

    /// Persist the inbound-sync checkpoint a backend returned for a folder,
    /// overwriting any prior value (a checkpoint is a running high-water mark,
    /// not history). Written only after a folder's messages have been fetched
    /// and persisted, so the stored checkpoint can never advance past work that
    /// actually landed in the store.
    pub async fn save_folder_sync_state(
        &self,
        account_id: &str,
        folder_id: &str,
        state: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO folder_sync_state (account_id, folder_id, state)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(account_id)
        .bind(folder_id)
        .bind(state)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;
        Ok(())
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
        // Encryption failures MUST surface before any SQL runs: a swallowed error here would
        // otherwise leave a row with an empty/placeholder body column, indistinguishable from
        // a genuinely empty body on read-back.
        let enc_plain = email
            .body_plain
            .as_ref()
            .map(|p| crate::cipher::PayloadCipher::encrypt_text_at_rest(&self.storage_key, p))
            .transpose()?;
        let enc_html = email
            .body_html
            .as_ref()
            .map(|h| crate::cipher::PayloadCipher::encrypt_text_at_rest(&self.storage_key, h))
            .transpose()?;

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

    /// Save a [`nuncio_core::model::CalendarEvent`] to SQLite (INSERT OR REPLACE).
    ///
    /// Unlike [`Self::save_email`], calendar event summary/location are never encrypted at
    /// rest (see the confidentiality note above the `events_fts` `CREATE VIRTUAL TABLE`
    /// statement in [`Self::migrate`]), so the `events_ai`/`events_ad`/`events_au` triggers
    /// created there keep `events_fts` in sync automatically -- there is no separate manual
    /// FTS write here the way there is for messages.
    pub async fn save_calendar_event(
        &self,
        event: &nuncio_core::model::CalendarEvent,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO calendar_events
            (id, account_id, calendar_id, summary, start_time, end_time, rrule, location)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&event.id)
        .bind(&event.account_id)
        .bind(&event.calendar_id)
        .bind(&event.summary)
        .bind(event.start_time)
        .bind(event.end_time)
        .bind(&event.rrule)
        .bind(&event.location)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
    }

    /// Query persisted calendar events for `account_id` whose `[start_time, end_time]` window
    /// overlaps `[start_window, end_window]` (inclusive, unix seconds) -- standard interval
    /// overlap, not "fully contained": an event that starts before `start_window` but is still
    /// ongoing, or one that starts within the window but ends after it, is still returned.
    #[allow(clippy::type_complexity)]
    pub async fn list_calendar_events(
        &self,
        account_id: &str,
        start_window: i64,
        end_window: i64,
    ) -> Result<Vec<nuncio_core::model::CalendarEvent>, DatabaseError> {
        let rows: Vec<(
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
            SELECT id, account_id, calendar_id, summary, start_time, end_time, rrule, location
            FROM calendar_events
            WHERE account_id = ? AND start_time <= ? AND end_time >= ?
            ORDER BY start_time ASC
            "#,
        )
        .bind(account_id)
        .bind(end_window)
        .bind(start_window)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(
                |(id, account_id, calendar_id, summary, start_time, end_time, rrule, location)| {
                    nuncio_core::model::CalendarEvent {
                        id,
                        account_id,
                        calendar_id,
                        summary,
                        start_time,
                        end_time,
                        rrule,
                        location,
                    }
                },
            )
            .collect())
    }

    /// Query every persisted recurring calendar event (non-null, non-empty `rrule`) for
    /// `account_id`, with **no** time-window filter -- a recurring master's own stored
    /// `start_time`/`end_time` can validly sit long before (or after) any given query
    /// window while its expanded occurrences still fall inside it, so callers that need
    /// to materialize occurrences for a window must expand every one of the account's
    /// recurring masters rather than pre-filtering by the master row's own timestamps.
    #[allow(clippy::type_complexity)]
    pub async fn list_recurring_calendar_events(
        &self,
        account_id: &str,
    ) -> Result<Vec<nuncio_core::model::CalendarEvent>, DatabaseError> {
        let rows: Vec<(
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
            SELECT id, account_id, calendar_id, summary, start_time, end_time, rrule, location
            FROM calendar_events
            WHERE account_id = ? AND rrule IS NOT NULL AND rrule != ''
            ORDER BY start_time ASC
            "#,
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(
                |(id, account_id, calendar_id, summary, start_time, end_time, rrule, location)| {
                    nuncio_core::model::CalendarEvent {
                        id,
                        account_id,
                        calendar_id,
                        summary,
                        start_time,
                        end_time,
                        rrule,
                        location,
                    }
                },
            )
            .collect())
    }

    /// Retrieve a single calendar event by ID.
    #[allow(clippy::type_complexity)]
    pub async fn get_calendar_event(
        &self,
        event_id: &str,
    ) -> Result<nuncio_core::model::CalendarEvent, DatabaseError> {
        let row: (
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
            SELECT id, account_id, calendar_id, summary, start_time, end_time, rrule, location
            FROM calendar_events
            WHERE id = ?
            "#,
        )
        .bind(event_id)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(nuncio_core::model::CalendarEvent {
            id: row.0,
            account_id: row.1,
            calendar_id: row.2,
            summary: row.3,
            start_time: row.4,
            end_time: row.5,
            rrule: row.6,
            location: row.7,
        })
    }

    /// Save a [`nuncio_contacts::Contact`] to SQLite (INSERT OR REPLACE).
    ///
    /// `nuncio-store` is otherwise limited to depending only on `nuncio-core` domain types;
    /// this method (and [`Self::list_contacts`]/[`Self::get_contact`]) is a deliberate,
    /// narrow exception, taking a direct dependency on `nuncio_contacts::Contact` because that
    /// type has not (yet) been promoted to `nuncio-core` the way `CalendarEvent`/`Email` have.
    pub async fn save_contact(
        &self,
        contact: &nuncio_contacts::Contact,
    ) -> Result<(), DatabaseError> {
        let emails_json =
            serde_json::to_string(&contact.emails).unwrap_or_else(|_| "[]".to_string());
        let phones_json =
            serde_json::to_string(&contact.phones).unwrap_or_else(|_| "[]".to_string());
        let last_interacted_str = contact.last_interacted_at.map(|t| t.to_rfc3339());

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO contacts
            (id, account_id, display_name, given_name, family_name, organization, job_title,
             notes, avatar_url, emails_json, phones_json, is_favorite, interaction_count,
             last_interacted_at, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&contact.id)
        .bind(&contact.account_id)
        .bind(&contact.display_name)
        .bind(&contact.given_name)
        .bind(&contact.family_name)
        .bind(&contact.organization)
        .bind(&contact.job_title)
        .bind(&contact.notes)
        .bind(&contact.avatar_url)
        .bind(&emails_json)
        .bind(&phones_json)
        .bind(contact.is_favorite)
        .bind(contact.interaction_count as i64)
        .bind(&last_interacted_str)
        .bind(contact.created_at.to_rfc3339())
        .bind(contact.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
    }

    /// Query persisted contacts belonging to `account_id`, most-recently-interacted first.
    ///
    /// See the confidentiality/dependency note on [`Self::save_contact`].
    pub async fn list_contacts(
        &self,
        account_id: &str,
    ) -> Result<Vec<nuncio_contacts::Contact>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT id, account_id, display_name, given_name, family_name, organization,
                   job_title, notes, avatar_url, emails_json, phones_json, is_favorite,
                   interaction_count, last_interacted_at, created_at, updated_at
            FROM contacts
            WHERE account_id = ?
            ORDER BY interaction_count DESC, display_name ASC
            "#,
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows.iter().map(contact_from_row).collect())
    }

    /// Retrieve a single contact by ID.
    ///
    /// See the confidentiality/dependency note on [`Self::save_contact`].
    pub async fn get_contact(
        &self,
        contact_id: &str,
    ) -> Result<nuncio_contacts::Contact, DatabaseError> {
        let row = sqlx::query(
            r#"
            SELECT id, account_id, display_name, given_name, family_name, organization,
                   job_title, notes, avatar_url, emails_json, phones_json, is_favorite,
                   interaction_count, last_interacted_at, created_at, updated_at
            FROM contacts
            WHERE id = ?
            "#,
        )
        .bind(contact_id)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(contact_from_row(&row))
    }

    /// Update a single message's read/unread flag in place.
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

    /// Query messages across the WHOLE store for export purposes,
    /// optionally narrowed to a single account or a
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
    /// result, rather than the opaque
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

    /// Proves that a transient error surfacing from the integrity probe (here, a
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

    /// End-to-end proof that a genuinely transient, operational condition (another
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

    fn sample_calendar_event(id: &str, account_id: &str) -> nuncio_core::model::CalendarEvent {
        nuncio_core::model::CalendarEvent {
            id: id.to_string(),
            account_id: account_id.to_string(),
            calendar_id: "cal-work".to_string(),
            summary: "Architecture Sync".to_string(),
            start_time: 1_700_000_000,
            end_time: 1_700_003_600,
            rrule: None,
            location: Some("Conference Room B".to_string()),
        }
    }

    /// A saved event is retrievable by both `get_calendar_event` and `list_calendar_events`,
    /// and shows up in `SearchEngine::search_events` -- proving the real write path (not a
    /// raw-SQL test insert) keeps the `events_fts` trigger-populated index in sync.
    #[tokio::test]
    async fn save_get_list_and_search_calendar_event() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let event = sample_calendar_event("evt-db-100", "acct-1");
        engine
            .save_calendar_event(&event)
            .await
            .expect("save calendar event succeeds");

        let fetched = engine
            .get_calendar_event("evt-db-100")
            .await
            .expect("get calendar event succeeds");
        assert_eq!(fetched, event);

        let listed = engine
            .list_calendar_events("acct-1", 1_699_000_000, 1_701_000_000)
            .await
            .expect("list calendar events succeeds");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], event);

        // Outside the queried window entirely -> no match.
        let out_of_window = engine
            .list_calendar_events("acct-1", 1_800_000_000, 1_801_000_000)
            .await
            .expect("list calendar events succeeds");
        assert!(out_of_window.is_empty());

        // Different account -> no match, even within the same window.
        let other_account = engine
            .list_calendar_events("acct-other", 1_699_000_000, 1_701_000_000)
            .await
            .expect("list calendar events succeeds");
        assert!(other_account.is_empty());

        let search = crate::search::SearchEngine::new(&engine);
        let hits = search
            .search_events("Architecture")
            .await
            .expect("search events succeeds");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "evt-db-100");
    }

    /// `list_recurring_calendar_events` returns only rows with a non-empty `rrule`, for the
    /// requested account only, and applies no time-window filter at all -- a master whose own
    /// stored `start_time` is far outside any given query window must still come back, since
    /// its expanded occurrences may fall inside that window.
    #[tokio::test]
    async fn list_recurring_calendar_events_filters_by_account_and_rrule_presence() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let mut recurring = sample_calendar_event("evt-recurring", "acct-1");
        recurring.rrule = Some("FREQ=WEEKLY;INTERVAL=1".to_string());
        recurring.start_time = 1_000; // far outside any window used below
        recurring.end_time = 4_600;
        engine.save_calendar_event(&recurring).await.unwrap();

        let non_recurring = sample_calendar_event("evt-single", "acct-1");
        engine.save_calendar_event(&non_recurring).await.unwrap();

        let mut other_account_recurring = sample_calendar_event("evt-other-acct", "acct-2");
        other_account_recurring.rrule = Some("FREQ=DAILY".to_string());
        engine
            .save_calendar_event(&other_account_recurring)
            .await
            .unwrap();

        let recurring_for_acct1 = engine
            .list_recurring_calendar_events("acct-1")
            .await
            .expect("list recurring calendar events succeeds");
        assert_eq!(recurring_for_acct1.len(), 1);
        assert_eq!(recurring_for_acct1[0].id, "evt-recurring");

        let recurring_for_acct2 = engine
            .list_recurring_calendar_events("acct-2")
            .await
            .expect("list recurring calendar events succeeds");
        assert_eq!(recurring_for_acct2.len(), 1);
        assert_eq!(recurring_for_acct2[0].id, "evt-other-acct");
    }

    /// Re-saving an event with the same ID (an update, not a fresh insert) replaces both the
    /// row and its FTS entry rather than duplicating either.
    #[tokio::test]
    async fn save_calendar_event_upserts_existing_row_and_fts_entry() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let mut event = sample_calendar_event("evt-db-200", "acct-1");
        engine.save_calendar_event(&event).await.unwrap();

        event.summary = "Renamed Planning Session".to_string();
        event.start_time = 1_750_000_000;
        event.end_time = 1_750_003_600;
        engine.save_calendar_event(&event).await.unwrap();

        let fetched = engine.get_calendar_event("evt-db-200").await.unwrap();
        assert_eq!(fetched.summary, "Renamed Planning Session");
        assert_eq!(fetched.start_time, 1_750_000_000);

        let search = crate::search::SearchEngine::new(&engine);
        assert!(search
            .search_events("Architecture")
            .await
            .unwrap()
            .is_empty());
        let hits = search.search_events("Renamed").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "evt-db-200");
    }

    /// `get_calendar_event` for an ID that was never saved reports
    /// `sqlx::Error::RowNotFound` rather than silently fabricating an empty/default event.
    #[tokio::test]
    async fn get_calendar_event_reports_not_found_for_missing_event() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let err = engine
            .get_calendar_event("evt-does-not-exist")
            .await
            .expect_err("missing event must be a real error");
        assert!(matches!(
            err,
            DatabaseError::Query(sqlx::Error::RowNotFound)
        ));
    }

    fn sample_contact(id: &str, account_id: &str) -> nuncio_contacts::Contact {
        let mut contact = nuncio_contacts::Contact::new("Architecture Contact", "arch@nuncio.mx");
        contact.id = id.to_string();
        contact.account_id = Some(account_id.to_string());
        contact.organization = Some("KofTwentyTwo".to_string());
        contact
    }

    /// Count rows in `contacts_fts` matching a trigram search term, proving the real write
    /// path (not a raw-SQL test insert) keeps the trigger-populated FTS index in sync -- the
    /// same proof pattern as `save_get_list_and_search_calendar_event`, without depending on
    /// `SearchEngine` gaining a contacts-specific method (out of scope for this story).
    async fn contacts_fts_hit_count(engine: &DatabaseEngine, term: &str) -> i64 {
        let pattern = format!("%{term}%");
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM contacts_fts WHERE display_name LIKE ?")
                .bind(&pattern)
                .fetch_one(engine.pool())
                .await
                .unwrap();
        row.0
    }

    /// A saved contact is retrievable by both `get_contact` and `list_contacts`, and its FTS
    /// mirror is kept in sync by the real write path (not a raw-SQL test insert).
    #[tokio::test]
    async fn save_get_list_and_search_contact() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let contact = sample_contact("ct-db-100", "acct-1");
        engine
            .save_contact(&contact)
            .await
            .expect("save contact succeeds");

        let fetched = engine
            .get_contact("ct-db-100")
            .await
            .expect("get contact succeeds");
        assert_eq!(fetched.display_name, contact.display_name);
        assert_eq!(fetched.account_id, contact.account_id);
        assert_eq!(fetched.emails[0].email, contact.emails[0].email);

        let listed = engine
            .list_contacts("acct-1")
            .await
            .expect("list contacts succeeds");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "ct-db-100");

        // Different account -> no match.
        let other_account = engine
            .list_contacts("acct-other")
            .await
            .expect("list contacts succeeds");
        assert!(other_account.is_empty());

        assert_eq!(contacts_fts_hit_count(&engine, "Architecture").await, 1);
    }

    /// Re-saving a contact with the same ID (an update, not a fresh insert) replaces both the
    /// row and its FTS entry rather than duplicating either.
    #[tokio::test]
    async fn save_contact_upserts_existing_row_and_fts_entry() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let mut contact = sample_contact("ct-db-200", "acct-1");
        engine.save_contact(&contact).await.unwrap();

        contact.display_name = "Renamed Contact".to_string();
        engine.save_contact(&contact).await.unwrap();

        let fetched = engine.get_contact("ct-db-200").await.unwrap();
        assert_eq!(fetched.display_name, "Renamed Contact");

        assert_eq!(
            contacts_fts_hit_count(&engine, "Architecture Contact").await,
            0
        );
        assert_eq!(contacts_fts_hit_count(&engine, "Renamed").await, 1);
    }

    /// `get_contact` for an ID that was never saved reports `sqlx::Error::RowNotFound` rather
    /// than silently fabricating an empty/default contact.
    #[tokio::test]
    async fn get_contact_reports_not_found_for_missing_contact() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let err = engine
            .get_contact("ct-does-not-exist")
            .await
            .expect_err("missing contact must be a real error");
        assert!(matches!(
            err,
            DatabaseError::Query(sqlx::Error::RowNotFound)
        ));
    }

    /// `set_message_read` flips the persisted
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

    /// Regression proof that the per-protocol TLS transport modes are
    /// genuinely persisted and round-tripped -- previously they were
    /// validated on add but silently discarded, with `list_accounts`
    /// hardcoding both back to `ImplicitTls` regardless of what was saved.
    /// Saves an account whose IMAP mode is `StartTls` and SMTP mode is
    /// `Plain`, then re-reads it via BOTH `list_accounts` and `get_account`
    /// and asserts each distinct mode survived rather than collapsing to the
    /// default.
    #[tokio::test]
    async fn account_tls_modes_survive_save_and_reload() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let acct = nuncio_core::AccountConfig {
            id: "acct-tls-1".to_string(),
            name: "Mixed TLS Account".to_string(),
            email_address: "mixed@nuncio.mx".to_string(),
            protocol: nuncio_core::AccountProtocol::ImapSmtp,
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 143,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 25,
            use_tls: false,
            imap_tls_mode: nuncio_core::TlsMode::StartTls,
            smtp_tls_mode: nuncio_core::TlsMode::Plain,
            keyring_secret_key: "nuncio/acct-tls-1".to_string(),
            sync_interval_secs: 60,
        };

        engine.save_account(&acct).await.expect("save succeeds");

        let listed = engine.list_accounts().await.expect("list succeeds");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].imap_tls_mode, nuncio_core::TlsMode::StartTls);
        assert_eq!(listed[0].smtp_tls_mode, nuncio_core::TlsMode::Plain);

        let fetched = engine
            .get_account("acct-tls-1")
            .await
            .expect("get succeeds")
            .expect("account present");
        assert_eq!(fetched.imap_tls_mode, nuncio_core::TlsMode::StartTls);
        assert_eq!(fetched.smtp_tls_mode, nuncio_core::TlsMode::Plain);
        assert_eq!(fetched, acct);
    }

    /// Proves `get_account` resolves a saved account by id and returns `None`
    /// for an unknown id, and that `delete_account` genuinely removes the row
    /// (and is idempotent for an already-absent id).
    #[tokio::test]
    async fn get_and_delete_account_round_trip() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let acct = nuncio_core::AccountConfig {
            id: "acct-del-1".to_string(),
            name: "Deletable".to_string(),
            email_address: "del@nuncio.mx".to_string(),
            protocol: nuncio_core::AccountProtocol::ImapSmtp,
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
            use_tls: true,
            imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            keyring_secret_key: "nuncio/acct-del-1".to_string(),
            sync_interval_secs: 60,
        };
        engine.save_account(&acct).await.expect("save succeeds");

        assert!(engine
            .get_account("acct-del-1")
            .await
            .expect("get succeeds")
            .is_some());
        assert!(engine
            .get_account("acct-missing")
            .await
            .expect("get succeeds")
            .is_none());

        engine
            .delete_account("acct-del-1")
            .await
            .expect("delete succeeds");
        assert!(engine
            .get_account("acct-del-1")
            .await
            .expect("get succeeds")
            .is_none());

        // Idempotent: deleting an already-absent id is not an error.
        engine
            .delete_account("acct-del-1")
            .await
            .expect("second delete is a no-op");
    }

    #[tokio::test]
    async fn folder_sync_state_round_trip_is_scoped_and_overwrites() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        // Absent before any sync completes.
        assert_eq!(
            engine
                .get_folder_sync_state("acct-1", "INBOX")
                .await
                .expect("get succeeds"),
            None
        );

        engine
            .save_folder_sync_state("acct-1", "INBOX", "105")
            .await
            .expect("save succeeds");
        assert_eq!(
            engine
                .get_folder_sync_state("acct-1", "INBOX")
                .await
                .expect("get succeeds"),
            Some("105".to_string())
        );

        // A later checkpoint overwrites (high-water mark, not history).
        engine
            .save_folder_sync_state("acct-1", "INBOX", "220")
            .await
            .expect("save succeeds");
        assert_eq!(
            engine
                .get_folder_sync_state("acct-1", "INBOX")
                .await
                .expect("get succeeds"),
            Some("220".to_string())
        );

        // Keyed by (account, folder): a different folder/account is unaffected.
        assert_eq!(
            engine
                .get_folder_sync_state("acct-1", "Sent")
                .await
                .expect("get succeeds"),
            None
        );
        assert_eq!(
            engine
                .get_folder_sync_state("acct-2", "INBOX")
                .await
                .expect("get succeeds"),
            None
        );
    }

    /// Proves the additive `smtp_host`/`smtp_port`
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
        // before the `smtp_host`/`smtp_port` columns existed would look like on disk.
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
