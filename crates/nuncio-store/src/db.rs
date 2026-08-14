//! SQLite database engine initialization, connection pooling, and migrations.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use tempfile::TempDir;
use thiserror::Error;
use zeroize::Zeroizing;

/// Raw row shape for a pending remote mutation record fetched from SQLite.
type PendingMutationRow = (String, String, String, String, String, String, i64, i64);

/// `mutation_conflicts` row: id, mutation_id, message_id, mutation_type, then the
/// four nullable placement coordinates, observed evidence, and resolution state.
type ConflictRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
    Option<i64>,
    Option<String>,
);

/// Rebuild a conflict from its row, reassembling the placement only when all
/// four coordinates are present -- a partial placement addresses nothing.
fn conflict_from_row(row: ConflictRow) -> nuncio_core::model::MutationConflict {
    let (
        id,
        mutation_id,
        message_id,
        mutation_type,
        account_id,
        folder_id,
        uid_validity,
        remote_id,
        observed,
        detected_at,
        resolved_at,
        resolution,
    ) = row;
    let placement = match (account_id, folder_id, uid_validity, remote_id) {
        (Some(account_id), Some(folder_id), Some(uid_validity), Some(remote_id)) => {
            Some(nuncio_core::model::PlacementKey {
                account_id,
                folder_id,
                uid_validity,
                remote_id,
            })
        }
        _ => None,
    };
    nuncio_core::model::MutationConflict {
        id,
        mutation_id,
        message_id,
        mutation_type,
        placement,
        observed,
        detected_at,
        resolved_at,
        resolution,
    }
}

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
    /// The database was written by a newer binary than this one.
    ///
    /// Fail-closed: the schema is built from `CREATE TABLE IF NOT EXISTS`, so
    /// an older binary would otherwise open a newer database successfully and
    /// write the old model into it, leaving two mutually invisible truths in
    /// one file. There is no safe way to continue.
    #[error(
        "database identity schema v{on_disk} is newer than this build supports (v{supported}); \
         upgrade nunciod rather than running an older binary against it"
    )]
    SchemaTooNew {
        /// The generation recorded in the database's `user_version`.
        on_disk: i64,
        /// The newest generation this binary understands.
        supported: i64,
    },
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
    /// A body payload failed AEAD decryption (or UTF-8/hex decoding) on read.
    /// This is a fail-closed error: it is returned instead of ever reading back a
    /// corrupted or tampered ciphertext column as a silently-empty body, which would
    /// defeat the AEAD's integrity guarantee exactly where it matters -- detecting
    /// tampering on read.
    #[error("payload decryption failed: {0}")]
    Decryption(String),
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
    storage_key: Zeroizing<[u8; 32]>,
    worm_key: Zeroizing<[u8; 32]>,
    ledger_key: Zeroizing<[u8; 32]>,
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
) -> Result<
    (
        Zeroizing<[u8; 32]>,
        Zeroizing<[u8; 32]>,
        Zeroizing<[u8; 32]>,
    ),
    DatabaseError,
> {
    let storage_key_bytes = secrets
        .get_or_create_key_bytes(crate::vault::STORAGE_KEY_ACCOUNT, 32)
        .map_err(|e| DatabaseError::KeyProvisioning(e.to_string()))?;
    let storage_key: [u8; 32] = storage_key_bytes.try_into().map_err(|_| {
        DatabaseError::KeyProvisioning("storage key material must be exactly 32 bytes".to_string())
    })?;
    let worm_key_bytes = secrets
        .get_or_create_key_bytes(crate::vault::WORM_KEY_ACCOUNT, 32)
        .map_err(|e| DatabaseError::KeyProvisioning(e.to_string()))?;
    let worm_key: [u8; 32] = worm_key_bytes.try_into().map_err(|_| {
        DatabaseError::KeyProvisioning(
            "WORM audit key material must be exactly 32 bytes".to_string(),
        )
    })?;
    let ledger_key_bytes = secrets
        .get_or_create_key_bytes(crate::vault::LEDGER_KEY_ACCOUNT, 32)
        .map_err(|e| DatabaseError::KeyProvisioning(e.to_string()))?;
    let ledger_key: [u8; 32] = ledger_key_bytes.try_into().map_err(|_| {
        DatabaseError::KeyProvisioning("ledger key material must be exactly 32 bytes".to_string())
    })?;
    Ok((
        Zeroizing::new(storage_key),
        Zeroizing::new(worm_key),
        Zeroizing::new(ledger_key),
    ))
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

/// What a [`DatabaseEngine::save_email_at`] call actually changed.
///
/// The two flags answer different questions and a caller usually needs both:
/// `message_is_new` gates work that should happen once per message (indexing,
/// side-effecting filter actions), `placement_is_new` gates work that should
/// happen once per mailbox occupancy (per-folder counters, per-placement
/// evaluation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The message identity had not been stored before this call.
    pub message_is_new: bool,
    /// This mailbox occupancy had not been stored before this call.
    pub placement_is_new: bool,
}

/// The four coordinates that address one mailbox occupancy.
///
/// Re-exported from `nuncio-core` rather than defined here: a protocol backend
/// reports these when a folder stops mentioning a message, and this store
/// deletes exactly the rows they name, so both crates must be able to name one
/// type. `nuncio-mail` must not depend on `nuncio-store`, which leaves `core`
/// as the only place both can reach.
pub use nuncio_core::model::PlacementKey;

/// A message paired with the mailbox occupancy it was read through.
///
/// The two travel together out of every folder-scoped read: the caller almost
/// always needs the per-mailbox facts (the read flag, the addressing UID) as
/// well as the message, and re-deriving them would cost a query per row.
///
/// Deliberately not called `PlacedMessage`: `nuncio_mail::PlacedMessage` is a
/// different type carrying a third field (the identity tier a backend derived
/// the key from), and the sync path imports both. One name for two shapes in
/// one call site is a trap worth a longer name to avoid.
pub type MessageWithPlacement = (nuncio_core::model::Email, nuncio_core::model::Placement);

/// Keyset cursor for [`DatabaseEngine::list_messages_page`]: the
/// `(received_at, message_key, uidvalidity, uid)` of the last row of a page.
///
/// It addresses a placement, not a message, because a message can occupy one
/// folder more than once -- see that method for why a message-only cursor
/// silently drops rows.
pub type MessagePageCursor = (i64, String, String, String);

/// Column order of the identity-only `messages` projection shared by every
/// whole-store read path: key, account, subject, sender, recipient,
/// received_at, body_plain, body_html, message_id, content_hash.
type MessageRow = (
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// [`MessageRow`] widened with the joined placement's `uidvalidity`, `uid` and
/// `read_flag`, in that order.
type MessageWithPlacementRow = (
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    i64,
);

/// What a [`DatabaseEngine::delete_placements`] call removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteOutcome {
    /// Mailbox occupancies removed.
    pub placements_removed: u64,
    /// Messages that lost their last placement and were removed with it.
    pub messages_reaped: u64,
}

/// How many keys one generated `IN (...)` list may carry.
///
/// SQLite caps a statement's bind parameters at `SQLITE_MAX_VARIABLE_NUMBER`,
/// 32766 in the bundled build. The widest key built here spends four
/// parameters per entry (account, folder, uidvalidity, uid), so 4000 keys is
/// 16000 parameters -- comfortably under the cap with room for the limit to
/// halve, or for a future key to grow a fifth column, without silently
/// becoming a runtime error again.
///
/// The cap is worth this margin because overrunning it is not a slow query but
/// a hard failure of the whole statement: the enclosing folder sync aborts, no
/// checkpoint is written, and every retry fails in exactly the same place.
const MAX_KEYS_PER_IN_LIST: usize = 4_000;

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

    /// Cheap liveness probe: runs `SELECT 1` against the pool and returns
    /// `Ok(())` only if it genuinely round-trips. Unlike `check_integrity`
    /// (a full `PRAGMA quick_check`), this is a constant-time readiness ping
    /// suitable for a health endpoint served on every status request.
    pub async fn ping(&self) -> Result<(), DatabaseError> {
        let _: (i64,) = sqlx::query_as("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        Ok(())
    }

    /// Total unread messages across every folder, read live from the store.
    ///
    /// Counted over placements, not messages: `\Seen` is per-mailbox, so a
    /// message sitting unread in two folders is genuinely two unread items to
    /// clear.
    pub async fn count_unread_messages(&self) -> Result<u64, DatabaseError> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM placements WHERE read_flag = 0")
                .fetch_one(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;
        Ok(count.max(0) as u64)
    }

    /// Number of outbound mutations still queued in the outbox (status
    /// `pending`) -- the outbox depth reported by the daemon's health surface.
    pub async fn count_pending_mutations(&self) -> Result<u64, DatabaseError> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pending_remote_mutations WHERE status = 'pending'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;
        Ok(count.max(0) as u64)
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

    /// The identity-schema generation this binary understands.
    ///
    /// Distinct from the additive column migrations elsewhere in this module.
    /// Those add nullable columns and leave every stored row meaningful. This
    /// counter tracks changes to what a stored message *is* -- how it is keyed
    /// and what a sync checkpoint means -- where old rows cannot be reinterpreted
    /// under the new model at all.
    ///
    /// Bump this only for a change of that kind, and only together with the
    /// reset it implies (see [`DatabaseEngine::reconcile_schema_version`]).
    ///
    /// Generation 3 is where the IMAP engine began asking for RFC 8474
    /// `EMAILID` and `X-GM-MSGID`. Every message a capable server holds now
    /// derives its key from a stronger identity tier, so no key stored under
    /// generation 2 still names the message it named. Letting that happen a row
    /// at a time -- as each resync repointed a placement -- would work, but the
    /// repoint reap is a safety net against stranded plaintext, not a migration
    /// mechanism, and it would leave the store in two identity models for as
    /// long as any folder went unsynced. Rebuilding once is the honest form of
    /// the same change.
    pub const IDENTITY_SCHEMA_VERSION: i64 = 3;

    /// Reconcile the on-disk identity-schema generation with this binary's.
    ///
    /// Three outcomes:
    ///
    /// - **Same version** (and the common case): nothing happens.
    /// - **Database older**: the cached message store is rebuilt and the account
    ///   re-syncs from the server. Accounts, credentials and filter rules are
    ///   preserved; cached mail, sync checkpoints and the execution ledger are
    ///   not. This is deliberate and pre-release only -- see ADR 0002. Message
    ///   identity cannot be recomputed for rows already stored, so the
    ///   alternative is a permanently two-tier store in which old messages are
    ///   invisible to every guarantee the new model provides. The server holds
    ///   the authoritative copy either way.
    /// - **Database newer**: refuse to open. A `CREATE TABLE IF NOT EXISTS`
    ///   schema means an older binary can open a newer database and quietly
    ///   write the old model into it, leaving two mutually invisible truths in
    ///   one file. Failing loudly is the only way that does not corrupt.
    pub async fn reconcile_schema_version(&self) -> Result<(), DatabaseError> {
        let (on_disk,): (i64,) = sqlx::query_as("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        if on_disk > Self::IDENTITY_SCHEMA_VERSION {
            return Err(DatabaseError::SchemaTooNew {
                on_disk,
                supported: Self::IDENTITY_SCHEMA_VERSION,
            });
        }

        // 0 is both "brand new database" and "predates this counter". Neither
        // has anything worth preserving that a re-sync will not restore, and
        // the reset is a no-op on an empty store.
        if on_disk < Self::IDENTITY_SCHEMA_VERSION {
            if on_disk > 0 {
                tracing::warn!(
                    on_disk,
                    supported = Self::IDENTITY_SCHEMA_VERSION,
                    "message identity schema changed: rebuilding the cached message store; \
                     accounts and rules are preserved and mail will re-sync from the server"
                );
            }
            self.reset_cached_message_store().await?;
            sqlx::query(&format!(
                "PRAGMA user_version = {}",
                Self::IDENTITY_SCHEMA_VERSION
            ))
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        }

        Ok(())
    }

    /// Drop everything derived from the server, keeping everything the user
    /// configured.
    ///
    /// Cleared: messages and their FTS index, mailbox placements, the filter
    /// fire-once ledger, per-folder sync checkpoints, and the filter execution
    /// ledger. The execution ledger goes because its hash chain is keyed on
    /// message ids that will no longer resolve, and a chain pointing at absent
    /// rows has no audit value -- reinitialising it is more honest than
    /// carrying a permanently mixed-namespace log.
    ///
    /// Kept: accounts, credentials (which live in the OS keyring regardless),
    /// filter rules, and the WORM audit records, whose triggers forbid deletion
    /// and whose entries do not reference message identity.
    async fn reset_cached_message_store(&self) -> Result<(), DatabaseError> {
        let mut tx = self.pool.begin().await.map_err(DatabaseError::Query)?;
        for statement in [
            "DELETE FROM messages",
            "DELETE FROM messages_fts",
            "DELETE FROM placements",
            "DELETE FROM filter_fired",
            "DELETE FROM folder_sync_state",
            "DELETE FROM filter_execution_logs",
        ] {
            sqlx::query(statement)
                .execute(&mut *tx)
                .await
                .map_err(DatabaseError::Query)?;
        }
        tx.commit().await.map_err(DatabaseError::Query)?;
        Ok(())
    }

    /// Atomically claim the right to run `rule_id`'s actions against
    /// `message_key`, returning `true` only for the claim that won.
    ///
    /// The claim is the fire-once guard for side-effecting actions. It is keyed
    /// on message identity rather than on a placement because the same message
    /// legitimately arrives in several folders -- and, under the old
    /// folder-scoped identity, each arrival looked like a different message and
    /// fired the rule again. `INSERT ... ON CONFLICT DO NOTHING` makes winning
    /// the claim a single atomic statement, so two passes racing over the same
    /// message cannot both act.
    ///
    /// A claim outlives the message it was made against. Nothing reaps
    /// `filter_fired` when the last placement goes, so a message deleted and
    /// later re-delivered byte for byte will not fire its rules a second time.
    /// That is deliberate: reaping alongside the message would hand a remote
    /// party a way to re-trigger third-party side effects at will -- a
    /// mailing-list message deleted and re-sent would `FORWARD` again -- and
    /// under content-addressed identity an identical redelivery genuinely *is*
    /// the same message, so declining to re-fire is the honest reading. The
    /// failure this leaves open ("a rule that should have fired again did not")
    /// is the safe direction for actions that reach outside the mailbox.
    ///
    /// The cost is a table that only grows, bounded by rules x messages ever
    /// seen at roughly 150 bytes a row, and cleared only when an identity
    /// schema bump resets `user_version` and rebuilds the derived tables.
    pub async fn claim_filter_fire(
        &self,
        rule_id: &str,
        message_key: &str,
    ) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "INSERT INTO filter_fired (rule_id, message_key, fired_at) VALUES (?, ?, ?)
             ON CONFLICT (rule_id, message_key) DO NOTHING",
        )
        .bind(rule_id)
        .bind(message_key)
        .bind(chrono::Utc::now().timestamp())
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(result.rows_affected() == 1)
    }

    /// Whether `rule_id` has already acted on `message_key`.
    ///
    /// Public rather than test-only: the claim ledger answers a question
    /// operators and the outbox both have a real reason to ask -- "did this rule
    /// already act on this message" -- and callers in other crates cannot reach
    /// the pool directly.
    pub async fn has_filter_fired(
        &self,
        rule_id: &str,
        message_key: &str,
    ) -> Result<bool, DatabaseError> {
        let found: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM filter_fired WHERE rule_id = ? AND message_key = ?")
                .bind(rule_id)
                .bind(message_key)
                .fetch_optional(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;

        Ok(found.is_some())
    }

    /// How many distinct (rule, message) fires the ledger records.
    ///
    /// Counterpart to [`Self::has_filter_fired`] for callers that need to assert
    /// on the ledger as a whole rather than one entry.
    pub async fn filter_fire_count(&self) -> Result<i64, DatabaseError> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM filter_fired")
            .fetch_one(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        Ok(count)
    }

    /// Execute initial database migrations creating core envelope tables.
    pub async fn migrate(&self) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            -- Identity and immutable content, one row per message. Where the
            -- message sits lives in `placements`; nothing here is folder-scoped.
            CREATE TABLE IF NOT EXISTS messages (
                message_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                sender TEXT NOT NULL,
                recipient TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                body_plain TEXT,
                body_html TEXT,
                -- Captured at ingest, not yet read. Both are nullable because
                -- neither is always available: `Message-ID` is SHOULD, not
                -- MUST (RFC 5322 3.6.4), and a headers-only or envelope-only
                -- fetch has no full octets to hash.
                message_id TEXT,
                content_hash TEXT,
                identity_source TEXT NOT NULL DEFAULT 'surrogate'
            );

            -- One row per (mailbox, message) occupancy. `uidvalidity` is in the
            -- primary key because UIDs restart at 1 after a bump: without it the
            -- first message of the new generation would overwrite the row of
            -- whichever old message shared its UID.
            CREATE TABLE IF NOT EXISTS placements (
                account_id TEXT NOT NULL,
                folder_id TEXT NOT NULL,
                uidvalidity TEXT NOT NULL,
                uid TEXT NOT NULL,
                message_key TEXT NOT NULL,
                read_flag INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account_id, folder_id, uidvalidity, uid)
            );

            -- Fire-once ledger for filter actions. Keyed on the message
            -- *identity*, not a placement, so a message arriving in a second
            -- folder does not re-fire a rule that already acted on it -- which
            -- matters most for FORWARD and CALL WEBHOOK, whose effects land on
            -- third parties and cannot be undone by convergence.
            CREATE TABLE IF NOT EXISTS filter_fired (
                rule_id TEXT NOT NULL,
                message_key TEXT NOT NULL,
                fired_at INTEGER NOT NULL,
                PRIMARY KEY (rule_id, message_key)
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
                -- Retained (with a default) for on-disk compatibility with
                -- rows written before `imap_tls_mode`/`smtp_tls_mode` became
                -- the authoritative transport-security setting; no code
                -- reads or writes this column anymore.
                use_tls INTEGER NOT NULL DEFAULT 1,
                keyring_secret_key TEXT NOT NULL,
                sync_interval_secs INTEGER NOT NULL,
                smtp_host TEXT,
                smtp_port INTEGER,
                imap_tls_mode TEXT NOT NULL DEFAULT 'implicit_tls',
                smtp_tls_mode TEXT NOT NULL DEFAULT 'implicit_tls',
                collection_url TEXT,
                -- Defaults to 0 (off), including for accounts written before
                -- the column existed. Filter execution is not safe to run on
                -- more than one daemon per account, and defaulting a shared
                -- account to "every daemon forwards" would be the wrong way
                -- to be wrong.
                filters_enabled INTEGER NOT NULL DEFAULT 0
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

            CREATE TABLE IF NOT EXISTS mutation_conflicts (
                id TEXT PRIMARY KEY NOT NULL,
                mutation_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                mutation_type TEXT NOT NULL,
                account_id TEXT,
                folder_id TEXT,
                uidvalidity TEXT,
                uid TEXT,
                observed TEXT NOT NULL,
                detected_at INTEGER NOT NULL,
                resolved_at INTEGER,
                resolution TEXT
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
            CREATE INDEX IF NOT EXISTS idx_placements_message ON placements(message_key);
            CREATE INDEX IF NOT EXISTS idx_placements_folder ON placements(account_id, folder_id);

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
            -- `DatabaseEngine::save_email_at` (see there) and via `backfill_message_fts` below for
            -- any pre-existing rows. This means the trigram index now contains
            -- plaintext-derived body text: the FTS index itself is NOT encrypted, so message
            -- body content is recoverable from `messages_fts` by anyone with filesystem access
            -- to the SQLite database, even though the `messages.body_plain` column remains
            -- ciphertext. Column-level body encryption therefore provides only limited
            -- confidentiality while search is enabled -- full body confidentiality (an
            -- encrypted search index, or whole-database encryption) is future work and is NOT
            -- provided today.
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                message_key UNINDEXED,
                subject,
                sender,
                body_plain,
                tokenize = 'trigram'
            );

            -- Subject/sender are never encrypted in `messages`, so a delete-only trigger is
            -- sufficient here: it just keeps the FTS index free of orphaned rows. Insertion of
            -- `messages_fts` content happens explicitly in `save_email_at`, never via an
            -- AFTER INSERT/UPDATE trigger, because such a trigger would only ever see the
            -- ciphertext body column.
            --
            -- This trigger MUST stay on `messages` and never move to `placements`. Dropping one
            -- placement of a message that still sits in another folder must leave the body
            -- searchable; only losing the last placement removes the `messages` row, and that is
            -- what reaps the FTS row here. Firing on `placements` instead would delete the index
            -- entry while the body remained -- and, inverted, leaving it off `messages` entirely
            -- would strand plaintext bodies in `messages_fts` after the message itself was gone.
            CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                DELETE FROM messages_fts WHERE message_key = old.message_key;
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

        // The statements above create the schema in full, so there is nothing to
        // upgrade: every column this engine reads is declared in its own
        // `CREATE TABLE IF NOT EXISTS`. Databases are created, never migrated.
        //
        // Reconciling the identity-schema generation is a different question and
        // still applies: it asks whether a store was written under a model this
        // binary can still interpret, and rebuilds (or refuses) accordingly.
        // Runs after the tables exist, since the rebuild deletes from them.
        self.reconcile_schema_version().await?;
        self.backfill_message_fts().await?;

        Ok(())
    }

    /// Backfill `messages_fts` for any `messages` row that does not yet have a matching FTS
    /// entry -- rows written directly by a process that bypassed `save_email`'s explicit
    /// index population. That is not a legacy concern: `SqliteRecoveryEngine::salvage`
    /// restores `messages` rows straight into the table, so a database that has been
    /// through recovery has content the trigram index has never seen, and search would
    /// silently miss it. Run automatically as part of [`DatabaseEngine::migrate`]
    /// (idempotent: a fully-indexed database performs no work). The stored `body_plain`
    /// column holds AES-256-GCM ciphertext, so each candidate
    /// row is decrypted with this engine's storage key before being written into the
    /// plaintext-derived trigram index -- see the confidentiality tradeoff documented above
    /// `messages_fts`'s `CREATE VIRTUAL TABLE` statement in [`DatabaseEngine::migrate`].
    async fn backfill_message_fts(&self) -> Result<(), DatabaseError> {
        let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT m.message_key, m.subject, m.sender, m.body_plain
            FROM messages m
            WHERE NOT EXISTS (SELECT 1 FROM messages_fts f WHERE f.message_key = m.message_key)
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        for (message_key, subject, sender, body_plain) in rows {
            let dec_plain = body_plain
                .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p))
                .transpose()
                .map_err(|e| DatabaseError::Decryption(e.to_string()))?
                .unwrap_or_default();

            sqlx::query(
                "INSERT INTO messages_fts (message_key, subject, sender, body_plain) VALUES (?, ?, ?, ?)",
            )
            .bind(&message_key)
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
        use nuncio_core::{TlsMode, Transport};

        let protocol_str = serde_json::to_string(&config.protocol()).unwrap_or_default();
        // Flatten the single active transport onto the physical columns. The
        // `protocol` column doubles as the transport-kind discriminator that
        // `list_accounts` reads back to reconstruct the exact variant. Columns
        // that a given transport does not use are written empty/zero (and a
        // default TLS mode for the NOT NULL `*_tls_mode` columns); they are
        // ignored on load for that transport.
        let (
            server_host,
            server_port,
            imap_tls_mode,
            smtp_host,
            smtp_port,
            smtp_tls_mode,
            collection_url,
        ) = match &config.transport {
            Transport::ImapSmtp(t) => (
                t.imap_host.clone(),
                t.imap_port,
                t.imap_tls_mode,
                t.smtp_host.clone(),
                t.smtp_port,
                t.smtp_tls_mode,
                String::new(),
            ),
            Transport::Jmap(t) => (
                t.endpoint_host.clone(),
                0u16,
                TlsMode::ImplicitTls,
                String::new(),
                0u16,
                TlsMode::ImplicitTls,
                String::new(),
            ),
            Transport::Dav(t) => (
                String::new(),
                0u16,
                TlsMode::ImplicitTls,
                String::new(),
                0u16,
                TlsMode::ImplicitTls,
                t.collection_url.clone(),
            ),
        };
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO accounts
            (id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs, smtp_host, smtp_port, imap_tls_mode, smtp_tls_mode, collection_url, filters_enabled)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&config.id)
        .bind(&config.name)
        .bind(&config.email_address)
        .bind(protocol_str)
        .bind(&server_host)
        .bind(i64::from(server_port))
        // `use_tls` is a retained-but-unread on-disk column (see the
        // `CREATE TABLE`'s comment) -- written as a fixed placeholder since
        // no `AccountConfig` field backs it anymore, satisfying its
        // pre-existing `NOT NULL` constraint on every accounts table
        // regardless of whether it was created before or after this column
        // stopped being read.
        .bind(1i64)
        .bind(&config.keyring_secret_key)
        .bind(config.sync_interval_secs as i64)
        .bind(&smtp_host)
        .bind(i64::from(smtp_port))
        .bind(tls_mode_to_db(imap_tls_mode))
        .bind(tls_mode_to_db(smtp_tls_mode))
        .bind(&collection_url)
        .bind(i64::from(config.filters_enabled))
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
            String,
            i64,
            Option<String>,
            Option<i64>,
            String,
            String,
            Option<String>,
            i64,
        )> = sqlx::query_as(
            r#"
            SELECT id, name, email_address, protocol, server_host, server_port, keyring_secret_key, sync_interval_secs, smtp_host, smtp_port, imap_tls_mode, smtp_tls_mode, collection_url, filters_enabled
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
                    keyring_secret_key,
                    sync_interval_secs,
                    smtp_host,
                    smtp_port,
                    imap_tls_mode,
                    smtp_tls_mode,
                    collection_url,
                    filters_enabled,
                )| {
                    let protocol = serde_json::from_str(&protocol_str)
                        .unwrap_or(nuncio_core::AccountProtocol::ImapSmtp);
                    // Reconstruct the exact transport variant from the
                    // `protocol` discriminator column, reading only the columns
                    // that variant uses (see `save_account`).
                    let transport = match protocol {
                        nuncio_core::AccountProtocol::ImapSmtp => {
                            // Backfill-safe fallback: a row written before
                            // `smtp_host`/`smtp_port` existed has `NULL` in both
                            // columns (see `Self::ensure_accounts_smtp_columns`).
                            // Rather than surface an incomplete/invalid config,
                            // fall back to the same host/port already used for
                            // IMAP -- the best available default for an account
                            // configured before outbound mail had its own
                            // endpoint.
                            let resolved_smtp_host =
                                smtp_host.unwrap_or_else(|| server_host.clone());
                            let resolved_smtp_port =
                                smtp_port.map(|p| p as u16).unwrap_or(server_port as u16);
                            nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                                imap_host: server_host,
                                imap_port: server_port as u16,
                                imap_tls_mode: tls_mode_from_db(&imap_tls_mode),
                                smtp_host: resolved_smtp_host,
                                smtp_port: resolved_smtp_port,
                                smtp_tls_mode: tls_mode_from_db(&smtp_tls_mode),
                            })
                        }
                        nuncio_core::AccountProtocol::Jmap => {
                            nuncio_core::Transport::Jmap(nuncio_core::JmapTransport {
                                endpoint_host: server_host,
                            })
                        }
                        nuncio_core::AccountProtocol::CalDav => {
                            nuncio_core::Transport::Dav(nuncio_core::DavTransport {
                                collection_url: collection_url.unwrap_or_default(),
                            })
                        }
                    };
                    nuncio_core::AccountConfig {
                        id,
                        name,
                        email_address,
                        keyring_secret_key,
                        sync_interval_secs: sync_interval_secs as u64,
                        filters_enabled: filters_enabled != 0,
                        transport,
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

    /// Persist a message's identity and content, and record that it occupies
    /// `placement`, in one transaction.
    ///
    /// The message row is **first-write-wins**: a second sighting of the same
    /// key -- which is the normal case for a message that also exists in another
    /// folder -- leaves the stored subject and body untouched. That is what
    /// keeps a message whose identity rests on `Message-ID` + content hash from
    /// being rewritten by a later fetch claiming the same key, and it is why the
    /// FTS row is written only on the insert that actually created the message.
    ///
    /// The placement row is upserted rather than ignored, because its read flag
    /// is genuinely mutable: `\Seen` changes in the mailbox and the store must
    /// follow it.
    ///
    /// The message body is encrypted (AES-256-GCM) before being written to the `messages`
    /// table, but the *plaintext* body is also indexed into the standalone `messages_fts`
    /// FTS5 table at this same write, before the plaintext is discarded. This is what makes
    /// body search functional at all: a trigger mirroring the encrypted column would only ever
    /// index ciphertext. See the confidentiality tradeoff documented above the `messages_fts`
    /// `CREATE VIRTUAL TABLE` statement in [`DatabaseEngine::migrate`] -- the FTS index itself
    /// is not encrypted, so this intentionally trades some body confidentiality for working
    /// search.
    pub async fn save_email_at(
        &self,
        email: &nuncio_core::model::Email,
        source: nuncio_core::model::IdentitySource,
        placement: &nuncio_core::model::Placement,
    ) -> Result<SaveOutcome, DatabaseError> {
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

        // `BEGIN IMMEDIATE` takes the write lock before the first statement runs. This
        // transaction reads (the placement existence check) and then writes what it read, and in
        // WAL mode a deferred transaction that upgrades to a writer after another connection has
        // committed aborts with SQLITE_BUSY_SNAPSHOT -- which `busy_timeout` does not retry,
        // because there is no lock to wait for, only a stale snapshot. Acquiring the lock up
        // front makes a concurrent save block instead of failing.
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(DatabaseError::Query)?;

        // Ask before writing. SQLite reports one row affected for both arms of
        // an upsert, so after the fact there is no way to tell an inserted
        // placement from an updated one -- and the caller needs that distinction
        // to decide whether this is a first arrival worth filtering.
        // The occupant's key is read, not just its existence, because the upsert below may
        // repoint this slot at a different message and the previous key is unrecoverable
        // afterwards.
        let occupant: Option<(String,)> = sqlx::query_as(
            "SELECT message_key FROM placements
             WHERE account_id = ? AND folder_id = ? AND uidvalidity = ? AND uid = ?",
        )
        .bind(&placement.account_id)
        .bind(&placement.folder_id)
        .bind(&placement.uid_validity)
        .bind(&placement.remote_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?;
        let placement_is_new = occupant.is_none();
        let displaced_key = occupant.map(|(key,)| key).filter(|key| *key != email.id);

        let inserted = sqlx::query(
            r#"
            INSERT INTO messages
            (message_key, account_id, subject, sender, recipient, received_at,
             body_plain, body_html, message_id, content_hash, identity_source)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (message_key) DO NOTHING
            "#,
        )
        .bind(&email.id)
        .bind(&email.account_id)
        .bind(&email.subject)
        .bind(&email.sender)
        .bind(&email.recipient)
        .bind(email.received_at)
        .bind(&enc_plain)
        .bind(&enc_html)
        .bind(&email.message_id)
        .bind(&email.content_hash)
        .bind(source.as_str())
        .execute(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?;

        let message_is_new = inserted.rows_affected() == 1;

        // Index only on the insert that created the message. Re-indexing on
        // every placement would either duplicate the row (FTS5 standalone tables
        // have no upsert) or rewrite it with a body the first-write-wins rule
        // just rejected.
        if message_is_new {
            sqlx::query(
                "INSERT INTO messages_fts (message_key, subject, sender, body_plain) VALUES (?, ?, ?, ?)",
            )
            .bind(&email.id)
            .bind(&email.subject)
            .bind(&email.sender)
            .bind(email.body_plain.as_deref().unwrap_or(""))
            .execute(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;
        }

        sqlx::query(
            r#"
            INSERT INTO placements
            (account_id, folder_id, uidvalidity, uid, message_key, read_flag)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT (account_id, folder_id, uidvalidity, uid)
            DO UPDATE SET message_key = excluded.message_key, read_flag = excluded.read_flag
            "#,
        )
        .bind(&placement.account_id)
        .bind(&placement.folder_id)
        .bind(&placement.uid_validity)
        .bind(&placement.remote_id)
        .bind(&email.id)
        .bind(if placement.read { 1i64 } else { 0i64 })
        .execute(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?;

        // Repointing a slot at a different message drops the displaced message's placement
        // without it ever passing through [`Self::delete_placements`]. If that was its last
        // one, nothing else will ever delete it -- and its decrypted body would stay readable
        // in `messages_fts` forever, since only the `messages_ad` trigger clears that index.
        // Reap it here on exactly the terms `delete_placements` uses: gone only when nothing
        // points at it any more.
        if let Some(displaced_key) = displaced_key {
            sqlx::query(
                "DELETE FROM messages WHERE message_key = ?
                 AND NOT EXISTS (SELECT 1 FROM placements WHERE message_key = ?)",
            )
            .bind(&displaced_key)
            .bind(&displaced_key)
            .execute(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;
        }

        tx.commit().await.map_err(DatabaseError::Query)?;

        Ok(SaveOutcome {
            message_is_new,
            placement_is_new,
        })
    }

    /// Remove mailbox occupancies, reaping any message left with none.
    ///
    /// A server that stops reporting a UID has told us the message left *that
    /// mailbox*, not that it ceased to exist -- a move reports exactly this in
    /// the source folder while the message continues in the destination. So the
    /// placement goes unconditionally and the message goes only when its last
    /// placement does.
    ///
    /// The reap is what keeps the plaintext FTS index honest. `messages_fts`
    /// holds decrypted bodies keyed on `message_key`, reaped by the
    /// `messages_ad` trigger on `messages`. Deleting the message row is
    /// therefore the only thing that clears the index: skip the reap and every
    /// fully-deleted message leaves its body readable to anyone with filesystem
    /// access to the database.
    pub async fn delete_placements(
        &self,
        keys: &[PlacementKey],
    ) -> Result<DeleteOutcome, DatabaseError> {
        if keys.is_empty() {
            return Ok(DeleteOutcome {
                placements_removed: 0,
                messages_reaped: 0,
            });
        }

        // `BEGIN IMMEDIATE` for the same reason as in [`Self::save_email_at`]: this reads each
        // placement's owning message key and then deletes based on what it read, and a deferred
        // WAL transaction that upgrades to a writer mid-flight aborts with SQLITE_BUSY_SNAPSHOT
        // rather than waiting out `busy_timeout`.
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(DatabaseError::Query)?;
        let mut placements_removed = 0u64;
        let mut touched: Vec<String> = Vec::with_capacity(keys.len());

        for key in keys {
            // Remember which message each placement pointed at before removing
            // it -- afterwards there is nothing left to join through.
            let owner: Option<(String,)> = sqlx::query_as(
                "SELECT message_key FROM placements
                 WHERE account_id = ? AND folder_id = ? AND uidvalidity = ? AND uid = ?",
            )
            .bind(&key.account_id)
            .bind(&key.folder_id)
            .bind(&key.uid_validity)
            .bind(&key.remote_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;

            let Some((message_key,)) = owner else {
                continue;
            };

            let result = sqlx::query(
                "DELETE FROM placements
                 WHERE account_id = ? AND folder_id = ? AND uidvalidity = ? AND uid = ?",
            )
            .bind(&key.account_id)
            .bind(&key.folder_id)
            .bind(&key.uid_validity)
            .bind(&key.remote_id)
            .execute(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;

            placements_removed += result.rows_affected();
            touched.push(message_key);
        }

        touched.sort_unstable();
        touched.dedup();

        let mut messages_reaped = 0u64;
        for message_key in touched {
            // Reap only where nothing is left pointing at the message. The
            // `messages_ad` trigger clears the FTS row as a consequence.
            let result = sqlx::query(
                "DELETE FROM messages WHERE message_key = ?
                 AND NOT EXISTS (SELECT 1 FROM placements WHERE message_key = ?)",
            )
            .bind(&message_key)
            .bind(&message_key)
            .execute(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;
            messages_reaped += result.rows_affected();
        }

        tx.commit().await.map_err(DatabaseError::Query)?;
        Ok(DeleteOutcome {
            placements_removed,
            messages_reaped,
        })
    }

    /// Every mailbox this message currently occupies.
    pub async fn placements_of(
        &self,
        message_key: &str,
    ) -> Result<Vec<nuncio_core::model::Placement>, DatabaseError> {
        let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
            "SELECT account_id, folder_id, uidvalidity, uid, read_flag
             FROM placements WHERE message_key = ? ORDER BY folder_id ASC",
        )
        .bind(message_key)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(
                |(account_id, folder_id, uid_validity, remote_id, read_flag)| {
                    nuncio_core::model::Placement {
                        account_id,
                        folder_id,
                        uid_validity,
                        remote_id,
                        read: read_flag != 0,
                    }
                },
            )
            .collect())
    }

    /// Messages occupying one folder of one account, newest first, paired with
    /// the placement they were found through.
    ///
    /// The placement travels with the message because the caller almost always
    /// needs the per-mailbox facts -- the read flag and the addressing UID --
    /// and re-deriving them would mean a second query per row.
    pub async fn list_messages(
        &self,
        account_id: &str,
        folder_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageWithPlacement>, DatabaseError> {
        let rows: Vec<MessageWithPlacementRow> = sqlx::query_as(
            r#"
            SELECT m.message_key, m.account_id, m.subject, m.sender, m.recipient,
                   m.received_at, m.body_plain, m.body_html, m.message_id, m.content_hash,
                   p.uidvalidity, p.uid, p.read_flag
            FROM messages m
            JOIN placements p ON p.message_key = m.message_key
            WHERE p.account_id = ? AND p.folder_id = ?
            ORDER BY m.received_at DESC
            LIMIT ?
            "#,
        )
        .bind(account_id)
        .bind(folder_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let dec_plain = row
                .6
                .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p))
                .transpose()
                .map_err(|e| DatabaseError::Decryption(e.to_string()))?;
            let dec_html = row
                .7
                .map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h))
                .transpose()
                .map_err(|e| DatabaseError::Decryption(e.to_string()))?;

            out.push((
                nuncio_core::model::Email {
                    id: row.0,
                    account_id: row.1,
                    subject: row.2,
                    sender: row.3,
                    recipient: row.4,
                    received_at: row.5,
                    body_plain: dec_plain,
                    body_html: dec_html,
                    attachments: Vec::new(),
                    message_id: row.8,
                    content_hash: row.9,
                },
                nuncio_core::model::Placement {
                    account_id: account_id.to_string(),
                    folder_id: folder_id.to_string(),
                    uid_validity: row.10,
                    remote_id: row.11,
                    read: row.12 != 0,
                },
            ));
        }
        Ok(out)
    }

    /// Keyset-paginated listing of one folder's placements, newest first
    /// (`received_at DESC, message_key DESC, uidvalidity DESC, uid DESC`), each
    /// paired with the message it holds. `after` is the cursor of the last row
    /// of the previous page; `None` starts from the newest. Fetches
    /// `page_size + 1` rows to detect a following page: returns at most
    /// `page_size` pairs plus the cursor of the last returned row when more
    /// remain (else `None`).
    ///
    /// The cursor carries the full placement address, not just the message
    /// coordinates, because the join is **not** one-to-one within a folder: the
    /// placements primary key is `(account, folder, uidvalidity, uid)`, so one
    /// message legitimately occupies a single mailbox twice -- two `COPY`s of
    /// the same mail into one folder land under different UIDs. With only
    /// `(received_at, message_key)` in the keyset the ordering is not total, and
    /// two such rows straddling a page boundary make the second unreachable:
    /// the next page resumes strictly after the message key and skips its twin.
    /// `uidvalidity`/`uid` are compared as the text they are stored as, which is
    /// an arbitrary but total order -- and matching the `ORDER BY` exactly is
    /// all a keyset needs.
    pub async fn list_messages_page(
        &self,
        account_id: &str,
        folder_id: &str,
        after: Option<MessagePageCursor>,
        page_size: usize,
    ) -> Result<(Vec<MessageWithPlacement>, Option<MessagePageCursor>), DatabaseError> {
        let fetch = page_size.saturating_add(1);
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT m.message_key, m.account_id, m.subject, m.sender, m.recipient, \
             m.received_at, m.body_plain, m.body_html, m.message_id, m.content_hash, \
             p.uidvalidity, p.uid, p.read_flag \
             FROM messages m JOIN placements p ON p.message_key = m.message_key \
             WHERE p.account_id = ",
        );
        builder.push_bind(account_id.to_string());
        builder.push(" AND p.folder_id = ");
        builder.push_bind(folder_id.to_string());
        if let Some((ts, key, uid_validity, remote_id)) = &after {
            builder.push(" AND (m.received_at < ");
            builder.push_bind(*ts);
            builder.push(" OR (m.received_at = ");
            builder.push_bind(*ts);
            builder.push(" AND m.message_key < ");
            builder.push_bind(key.clone());
            builder.push(") OR (m.received_at = ");
            builder.push_bind(*ts);
            builder.push(" AND m.message_key = ");
            builder.push_bind(key.clone());
            builder.push(" AND p.uidvalidity < ");
            builder.push_bind(uid_validity.clone());
            builder.push(") OR (m.received_at = ");
            builder.push_bind(*ts);
            builder.push(" AND m.message_key = ");
            builder.push_bind(key.clone());
            builder.push(" AND p.uidvalidity = ");
            builder.push_bind(uid_validity.clone());
            builder.push(" AND p.uid < ");
            builder.push_bind(remote_id.clone());
            builder.push("))");
        }
        builder.push(
            " ORDER BY m.received_at DESC, m.message_key DESC, \
             p.uidvalidity DESC, p.uid DESC LIMIT ",
        );
        builder.push_bind(fetch as i64);

        let rows = builder
            .build_query_as::<MessageWithPlacementRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        let has_more = rows.len() > page_size;
        let mut placed = Vec::with_capacity(rows.len().min(page_size));
        for row in rows.into_iter().take(page_size) {
            let dec_plain = row
                .6
                .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p))
                .transpose()
                .map_err(|e| DatabaseError::Decryption(e.to_string()))?;
            let dec_html = row
                .7
                .map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h))
                .transpose()
                .map_err(|e| DatabaseError::Decryption(e.to_string()))?;
            placed.push((
                nuncio_core::model::Email {
                    id: row.0,
                    account_id: row.1,
                    subject: row.2,
                    sender: row.3,
                    recipient: row.4,
                    received_at: row.5,
                    body_plain: dec_plain,
                    body_html: dec_html,
                    attachments: Vec::new(),
                    message_id: row.8,
                    content_hash: row.9,
                },
                nuncio_core::model::Placement {
                    account_id: account_id.to_string(),
                    folder_id: folder_id.to_string(),
                    uid_validity: row.10,
                    remote_id: row.11,
                    read: row.12 != 0,
                },
            ));
        }

        let next = if has_more {
            placed.last().map(|(e, p)| {
                (
                    e.received_at,
                    e.id.clone(),
                    p.uid_validity.clone(),
                    p.remote_id.clone(),
                )
            })
        } else {
            None
        };
        placed.shrink_to_fit();
        Ok((placed, next))
    }

    /// Retrieve a message's identity and content by its key.
    ///
    /// Returns the message alone. Where it sits, and whether it is read there,
    /// are per-mailbox facts -- ask [`Self::placements_of`] for those.
    pub async fn get_message(
        &self,
        message_key: &str,
    ) -> Result<nuncio_core::model::Email, DatabaseError> {
        let row: MessageRow = sqlx::query_as(
            r#"
            SELECT message_key, account_id, subject, sender, recipient, received_at,
                   body_plain, body_html, message_id, content_hash
            FROM messages
            WHERE message_key = ?
            "#,
        )
        .bind(message_key)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let dec_plain = row
            .6
            .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p))
            .transpose()
            .map_err(|e| DatabaseError::Decryption(e.to_string()))?;
        let dec_html = row
            .7
            .map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h))
            .transpose()
            .map_err(|e| DatabaseError::Decryption(e.to_string()))?;

        Ok(nuncio_core::model::Email {
            id: row.0,
            account_id: row.1,
            subject: row.2,
            sender: row.3,
            recipient: row.4,
            received_at: row.5,
            body_plain: dec_plain,
            body_html: dec_html,
            attachments: Vec::new(),
            message_id: row.8,
            content_hash: row.9,
        })
    }

    /// Every placement currently stored for one folder of one account.
    ///
    /// The local half of a UID-set diff. This returns placements rather than
    /// message keys because what a server stops reporting is an occupancy, not a
    /// message: the same mail may still exist in another folder and must survive
    /// there.
    pub async fn placements_in_folder(
        &self,
        account_id: &str,
        folder_id: &str,
    ) -> Result<Vec<PlacementKey>, DatabaseError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT uidvalidity, uid FROM placements WHERE account_id = ? AND folder_id = ?",
        )
        .bind(account_id)
        .bind(folder_id)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(uid_validity, remote_id)| PlacementKey {
                account_id: account_id.to_string(),
                folder_id: folder_id.to_string(),
                uid_validity,
                remote_id,
            })
            .collect())
    }

    /// The subset of `keys` already stored.
    ///
    /// SQLite supports row-value `IN` lists, so a whole batch is one query
    /// rather than one existence check per placement. An empty slice
    /// short-circuits, since `IN ()` is not valid SQL.
    ///
    /// The batch is split internally at [`MAX_KEYS_PER_IN_LIST`] and the
    /// results unioned, so callers may pass an arbitrarily large set: an entire
    /// mailbox's worth of placements is exactly what a first sync produces, and
    /// pushing the split onto callers means the ceiling is only ever one
    /// forgetful caller away from aborting every retry of that sync
    /// identically. Chunking here makes the limit unreachable from outside.
    pub async fn existing_placements(
        &self,
        keys: &[PlacementKey],
    ) -> Result<std::collections::HashSet<PlacementKey>, DatabaseError> {
        let mut found = std::collections::HashSet::new();

        for chunk in keys.chunks(MAX_KEYS_PER_IN_LIST) {
            let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
                "SELECT account_id, folder_id, uidvalidity, uid FROM placements
                 WHERE (account_id, folder_id, uidvalidity, uid) IN (",
            );
            for (index, key) in chunk.iter().enumerate() {
                if index > 0 {
                    builder.push(", ");
                }
                builder.push("(");
                builder.push_bind(key.account_id.clone());
                builder.push(", ");
                builder.push_bind(key.folder_id.clone());
                builder.push(", ");
                builder.push_bind(key.uid_validity.clone());
                builder.push(", ");
                builder.push_bind(key.remote_id.clone());
                builder.push(")");
            }
            builder.push(")");

            let rows: Vec<(String, String, String, String)> = builder
                .build_query_as()
                .fetch_all(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;

            found.extend(rows.into_iter().map(
                |(account_id, folder_id, uid_validity, remote_id)| PlacementKey {
                    account_id,
                    folder_id,
                    uid_validity,
                    remote_id,
                },
            ));
        }

        Ok(found)
    }

    /// Return the subset of `keys` whose message identity is already present in the
    /// `messages` table, via `SELECT message_key FROM messages WHERE message_key
    /// IN (...)`.
    ///
    /// Lets a caller classify a whole fetched batch as new-vs-seen with one round trip instead
    /// of one existence lookup per key. Answers a strictly different question from
    /// [`Self::existing_placements`]: a message already known from another folder is *not* new
    /// here, but its arrival in this folder still is. Passing an empty slice short-circuits to
    /// an empty set without querying at all, since `IN ()` is not valid SQL.
    ///
    /// Chunked at [`MAX_KEYS_PER_IN_LIST`] for the same reason as
    /// [`Self::existing_placements`]: the bind-parameter ceiling is a property
    /// of the statement, so the only place it can be enforced once and for all
    /// is where the statement is built.
    pub async fn existing_message_ids(
        &self,
        keys: &[String],
    ) -> Result<std::collections::HashSet<String>, DatabaseError> {
        let mut found = std::collections::HashSet::new();

        for chunk in keys.chunks(MAX_KEYS_PER_IN_LIST) {
            let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> =
                sqlx::QueryBuilder::new("SELECT message_key FROM messages WHERE message_key IN (");
            let mut separated = builder.separated(", ");
            for key in chunk {
                separated.push_bind(key);
            }
            builder.push(")");

            let rows: Vec<(String,)> = builder
                .build_query_as()
                .fetch_all(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;

            found.extend(rows.into_iter().map(|(key,)| key));
        }

        Ok(found)
    }

    /// Save a [`nuncio_core::model::CalendarEvent`] to SQLite (INSERT OR REPLACE).
    ///
    /// Unlike [`Self::save_email_at`], calendar event summary/location are never encrypted at
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

    /// Keyset-paginated listing of `account_id`'s contacts, in the same order
    /// as [`Self::list_contacts`] (`interaction_count DESC, display_name ASC,
    /// id ASC`). `after` is the `(interaction_count, display_name, id)` of the
    /// last contact of the previous page. Fetches `page_size + 1` rows to
    /// detect a following page and returns the `(interaction_count,
    /// display_name, id)` cursor of the last returned contact when more remain.
    ///
    /// See the confidentiality/dependency note on [`Self::save_contact`].
    #[allow(clippy::type_complexity)]
    pub async fn list_contacts_page(
        &self,
        account_id: &str,
        after: Option<(i64, String, String)>,
        page_size: usize,
    ) -> Result<(Vec<nuncio_contacts::Contact>, Option<(i64, String, String)>), DatabaseError> {
        let fetch = page_size.saturating_add(1);
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT id, account_id, display_name, given_name, family_name, organization, \
             job_title, notes, avatar_url, emails_json, phones_json, is_favorite, \
             interaction_count, last_interacted_at, created_at, updated_at \
             FROM contacts WHERE account_id = ",
        );
        builder.push_bind(account_id.to_string());
        if let Some((ic, dn, id)) = &after {
            builder.push(" AND (interaction_count < ");
            builder.push_bind(*ic);
            builder.push(" OR (interaction_count = ");
            builder.push_bind(*ic);
            builder.push(" AND (display_name > ");
            builder.push_bind(dn.clone());
            builder.push(" OR (display_name = ");
            builder.push_bind(dn.clone());
            builder.push(" AND id > ");
            builder.push_bind(id.clone());
            builder.push("))))");
        }
        builder.push(" ORDER BY interaction_count DESC, display_name ASC, id ASC LIMIT ");
        builder.push_bind(fetch as i64);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        let has_more = rows.len() > page_size;
        let contacts: Vec<nuncio_contacts::Contact> =
            rows.iter().take(page_size).map(contact_from_row).collect();

        let next = if has_more {
            contacts.last().map(|c| {
                (
                    c.interaction_count as i64,
                    c.display_name.clone(),
                    c.id.clone(),
                )
            })
        } else {
            None
        };
        Ok((contacts, next))
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

    /// Update one placement's read flag in place.
    ///
    /// Scoped to a single mailbox occupancy because IMAP `\Seen` is per-mailbox:
    /// a message read in `INBOX` is not thereby read in `Archive`. Returns
    /// `DatabaseError::Query(sqlx::Error::RowNotFound)` when no such placement
    /// exists, so callers can still distinguish "flag flipped" from "never
    /// there" rather than silently succeeding on a no-op update.
    pub async fn set_placement_read(
        &self,
        key: &PlacementKey,
        read: bool,
    ) -> Result<(), DatabaseError> {
        let result = sqlx::query(
            "UPDATE placements SET read_flag = ?
             WHERE account_id = ? AND folder_id = ? AND uidvalidity = ? AND uid = ?",
        )
        .bind(if read { 1i64 } else { 0i64 })
        .bind(&key.account_id)
        .bind(&key.folder_id)
        .bind(&key.uid_validity)
        .bind(&key.remote_id)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        if result.rows_affected() == 0 {
            return Err(DatabaseError::Query(sqlx::Error::RowNotFound));
        }
        Ok(())
    }

    /// Query one account's folders with message counts.
    ///
    /// Folders are derived from the placements that reference them rather than
    /// stored in their own table, so a folder exists exactly as long as it holds
    /// mail. Scoping by account is required, not cosmetic: folder ids are not
    /// globally unique -- every account has an `INBOX` -- and grouping across
    /// accounts would report one merged folder holding both accounts' mail.
    pub async fn list_folders(
        &self,
        account_id: &str,
    ) -> Result<Vec<nuncio_core::model::Folder>, DatabaseError> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            r#"
            SELECT folder_id, COUNT(*) as total,
                   SUM(CASE WHEN read_flag = 0 THEN 1 ELSE 0 END) as unread
            FROM placements
            WHERE account_id = ?
            GROUP BY folder_id
            ORDER BY folder_id ASC
            "#,
        )
        .bind(account_id)
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

    /// Keyset-paginated listing of one account's folders with message counts,
    /// folder id ascending. `after` is the id of the last folder of the previous
    /// page. Fetches `page_size + 1` groups to detect a following page and
    /// returns the id of the last returned folder as the cursor when more
    /// remain. Account-scoped for the same reason as [`Self::list_folders`].
    pub async fn list_folders_page(
        &self,
        account_id: &str,
        after: Option<String>,
        page_size: usize,
    ) -> Result<(Vec<nuncio_core::model::Folder>, Option<String>), DatabaseError> {
        let fetch = page_size.saturating_add(1);
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT folder_id, COUNT(*) as total, SUM(CASE WHEN read_flag = 0 THEN 1 ELSE 0 END) as unread FROM placements WHERE account_id = ",
        );
        builder.push_bind(account_id.to_string());
        if let Some(id) = &after {
            builder.push(" AND folder_id > ");
            builder.push_bind(id.clone());
        }
        builder.push(" GROUP BY folder_id ORDER BY folder_id ASC LIMIT ");
        builder.push_bind(fetch as i64);

        let rows = builder
            .build_query_as::<(String, i64, i64)>()
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        let has_more = rows.len() > page_size;
        let folders: Vec<nuncio_core::model::Folder> = rows
            .into_iter()
            .take(page_size)
            .map(|(folder_id, total, unread)| nuncio_core::model::Folder {
                id: folder_id.clone(),
                name: folder_id,
                total_messages: total as usize,
                unread_messages: unread as usize,
            })
            .collect();

        let next = if has_more {
            folders.last().map(|f| f.id.clone())
        } else {
            None
        };
        Ok((folders, next))
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
            match nuncio_filter::NsqlParser::parse_rule(&name, priority as i32, &nsql_text) {
                Ok(mut parsed) => {
                    parsed.id = id;
                    parsed.enabled = enabled != 0;
                    parsed.created_at = created_at;
                    parsed.updated_at = updated_at;
                    rules.push(parsed);
                }
                Err(err) => {
                    // A stored rule that no longer parses (e.g. the NSQL grammar
                    // changed, or the row was hand-edited) must not vanish from
                    // enforcement without a trace: that silently disables the
                    // rule's protection with no operator-visible signal. Warn
                    // loudly with the rule id and skip it rather than either
                    // dropping it unnoticed or failing the whole listing over
                    // one bad row.
                    tracing::warn!(
                        rule_id = %id,
                        rule_name = %name,
                        error = %err,
                        "stored filter rule failed to parse and will not be enforced"
                    );
                }
            }
        }
        Ok(rules)
    }

    /// Keyset-paginated listing of persisted filter rules, `priority ASC, id
    /// ASC`. `after` is the `(priority, id)` of the last rule of the previous
    /// page. Fetches `page_size + 1` rows to detect a following page.
    ///
    /// The following-page detection and the returned cursor are computed from
    /// the RAW rows (their stored `(priority, id)`), before parsing -- a row
    /// whose stored NSQL no longer parses is skipped from the returned rules
    /// exactly as [`Self::list_filter_rules`] does, but it still counts toward
    /// the page boundary, so a page with skipped rows still advances the
    /// cursor correctly and never re-emits or gaps around a rule.
    #[allow(clippy::type_complexity)]
    pub async fn list_filter_rules_page(
        &self,
        after: Option<(i32, String)>,
        page_size: usize,
    ) -> Result<(Vec<nuncio_filter::FilterRule>, Option<(i32, String)>), DatabaseError> {
        let fetch = page_size.saturating_add(1);
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT id, name, priority, enabled, nsql_text, created_at, updated_at FROM filter_rules ",
        );
        if let Some((priority, id)) = &after {
            builder.push("WHERE (priority > ");
            builder.push_bind(*priority as i64);
            builder.push(" OR (priority = ");
            builder.push_bind(*priority as i64);
            builder.push(" AND id > ");
            builder.push_bind(id.clone());
            builder.push("))");
        }
        builder.push(" ORDER BY priority ASC, id ASC LIMIT ");
        builder.push_bind(fetch as i64);

        let rows = builder
            .build_query_as::<(String, String, i64, i64, String, i64, i64)>()
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        let has_more = rows.len() > page_size;
        let page: Vec<(String, String, i64, i64, String, i64, i64)> =
            rows.into_iter().take(page_size).collect();

        let next = if has_more {
            page.last().map(|r| (r.2 as i32, r.0.clone()))
        } else {
            None
        };

        let mut rules = Vec::new();
        for (id, name, priority, enabled, nsql_text, created_at, updated_at) in page {
            match nuncio_filter::NsqlParser::parse_rule(&name, priority as i32, &nsql_text) {
                Ok(mut parsed) => {
                    parsed.id = id;
                    parsed.enabled = enabled != 0;
                    parsed.created_at = created_at;
                    parsed.updated_at = updated_at;
                    rules.push(parsed);
                }
                Err(err) => {
                    tracing::warn!(
                        rule_id = %id,
                        rule_name = %name,
                        error = %err,
                        "stored filter rule failed to parse and will not be enforced"
                    );
                }
            }
        }
        Ok((rules, next))
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

    /// Record a mutation conflict as durable, queryable state.
    ///
    /// `INSERT OR REPLACE` keyed on the conflict id makes a re-record of the
    /// same conflict idempotent, so a drain pass that is retried after a crash
    /// cannot multiply one refusal into several rows a human has to dismiss.
    pub async fn record_conflict(
        &self,
        conflict: &nuncio_core::model::MutationConflict,
    ) -> Result<(), DatabaseError> {
        let placement = conflict.placement.as_ref();
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO mutation_conflicts
            (id, mutation_id, message_id, mutation_type, account_id, folder_id,
             uidvalidity, uid, observed, detected_at, resolved_at, resolution)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&conflict.id)
        .bind(&conflict.mutation_id)
        .bind(&conflict.message_id)
        .bind(&conflict.mutation_type)
        .bind(placement.map(|p| p.account_id.clone()))
        .bind(placement.map(|p| p.folder_id.clone()))
        .bind(placement.map(|p| p.uid_validity.clone()))
        .bind(placement.map(|p| p.remote_id.clone()))
        .bind(&conflict.observed)
        .bind(conflict.detected_at)
        .bind(conflict.resolved_at)
        .bind(conflict.resolution.as_deref())
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;
        Ok(())
    }

    /// List recorded mutation conflicts, newest first.
    ///
    /// Resolved conflicts stay in the table -- they are the audit trail of what
    /// a human decided -- so they are only returned when explicitly asked for.
    pub async fn list_conflicts(
        &self,
        include_resolved: bool,
        limit: usize,
    ) -> Result<Vec<nuncio_core::model::MutationConflict>, DatabaseError> {
        let sql = if include_resolved {
            r#"
            SELECT id, mutation_id, message_id, mutation_type, account_id, folder_id,
                   uidvalidity, uid, observed, detected_at, resolved_at, resolution
            FROM mutation_conflicts
            ORDER BY detected_at DESC, id ASC
            LIMIT ?
            "#
        } else {
            r#"
            SELECT id, mutation_id, message_id, mutation_type, account_id, folder_id,
                   uidvalidity, uid, observed, detected_at, resolved_at, resolution
            FROM mutation_conflicts
            WHERE resolved_at IS NULL
            ORDER BY detected_at DESC, id ASC
            LIMIT ?
            "#
        };
        let rows: Vec<ConflictRow> = sqlx::query_as(sql)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        Ok(rows.into_iter().map(conflict_from_row).collect())
    }

    /// Mark a conflict resolved. Returns `false` when no such unresolved
    /// conflict exists, so a caller can tell "already handled" from "applied"
    /// instead of reporting a success that never happened.
    pub async fn resolve_conflict(
        &self,
        id: &str,
        resolution: &str,
        resolved_at: i64,
    ) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            r#"
            UPDATE mutation_conflicts
            SET resolved_at = ?, resolution = ?
            WHERE id = ? AND resolved_at IS NULL
            "#,
        )
        .bind(resolved_at)
        .bind(resolution)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;
        Ok(result.rows_affected() > 0)
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
    ///
    /// The read of the latest chain hash and the dependent insert run inside a single
    /// `BEGIN IMMEDIATE` transaction: `BEGIN IMMEDIATE` acquires SQLite's write lock before
    /// any statement runs, so a second concurrent append (from another pooled connection,
    /// since `DatabaseEngine` is shared as an `Arc` across the daemon) blocks until this one
    /// commits rather than reading the same prev-hash and forking the chain.
    pub async fn save_filter_execution_log(
        &self,
        rule_id: &str,
        message_id: &str,
        action_taken: &str,
    ) -> Result<nuncio_filter::FilterExecutionLog, DatabaseError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(DatabaseError::Query)?;

        let latest_hash: Option<(String,)> =
            sqlx::query_as("SELECT hash FROM filter_execution_logs ORDER BY id DESC LIMIT 1")
                .fetch_optional(&mut *tx)
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
            &self.ledger_key[..],
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
        .execute(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?
        .last_insert_rowid();

        tx.commit().await.map_err(DatabaseError::Query)?;

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
                &self.ledger_key[..],
            );
            if computed != hash {
                return Ok(false);
            }
            expected_prev = hash;
        }

        Ok(true)
    }

    /// Fetch a page of messages using Keyset Chunking
    /// (`WHERE message_key > ? ORDER BY message_key ASC LIMIT ?`).
    ///
    /// Walks message identities, not placements: a chunked pass over the store
    /// wants each message exactly once, however many folders it occupies.
    pub async fn get_message_chunk(
        &self,
        last_key: &str,
        limit: usize,
    ) -> Result<Vec<nuncio_core::model::Email>, DatabaseError> {
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT message_key, account_id, subject, sender, recipient, received_at, \
             body_plain, body_html, message_id, content_hash FROM messages ",
        );
        if !last_key.is_empty() {
            builder.push("WHERE message_key > ");
            builder.push_bind(last_key);
        }
        builder.push(" ORDER BY message_key ASC LIMIT ");
        builder.push_bind(limit as i64);

        let query = builder.build_query_as::<MessageRow>();

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        rows.into_iter().map(|r| self.email_from_row(r)).collect()
    }

    /// Query messages across the WHOLE store for export purposes,
    /// optionally narrowed to a single account or a
    /// single folder. Passing `None` for both returns every message in the
    /// store. Ordered by `message_key` ascending, mirroring
    /// [`Self::get_message_chunk`]'s deterministic keyset ordering.
    ///
    /// The folder filter is an `EXISTS` over `placements` rather than a join:
    /// a message occupying two folders must still export as one message, and a
    /// join would emit it once per matching placement.
    pub async fn list_messages_for_export(
        &self,
        account_id: Option<&str>,
        folder_id: Option<&str>,
    ) -> Result<Vec<nuncio_core::model::Email>, DatabaseError> {
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT m.message_key, m.account_id, m.subject, m.sender, m.recipient, \
             m.received_at, m.body_plain, m.body_html, m.message_id, m.content_hash \
             FROM messages m",
        );

        let mut has_filter = false;
        if let Some(account_id) = account_id {
            builder.push(" WHERE m.account_id = ");
            builder.push_bind(account_id);
            has_filter = true;
        }
        if let Some(folder_id) = folder_id {
            builder.push(if has_filter { " AND " } else { " WHERE " });
            // Scoped to the message's own account, not just the folder name: folder ids are not
            // globally unique, so an unscoped `p.folder_id` match would export another account's
            // INBOX alongside this one -- the very collision this model exists to remove.
            builder.push(
                "EXISTS (SELECT 1 FROM placements p \
                 WHERE p.message_key = m.message_key \
                 AND p.account_id = m.account_id \
                 AND p.folder_id = ",
            );
            builder.push_bind(folder_id);
            builder.push(")");
        }
        builder.push(" ORDER BY m.message_key ASC");

        let query = builder.build_query_as::<MessageRow>();

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        rows.into_iter().map(|r| self.email_from_row(r)).collect()
    }

    /// Rebuild an [`nuncio_core::model::Email`] from the identity-column row
    /// shape shared by the whole-store read paths, decrypting the body columns
    /// with this engine's storage key.
    ///
    /// Decryption failures propagate as [`DatabaseError::Decryption`] rather
    /// than degrading to an empty body: a body that will not authenticate has
    /// been tampered with or corrupted, and silently returning nothing would
    /// hide exactly the event the AEAD exists to detect.
    fn email_from_row(&self, row: MessageRow) -> Result<nuncio_core::model::Email, DatabaseError> {
        let dec_plain = row
            .6
            .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p))
            .transpose()
            .map_err(|e| DatabaseError::Decryption(e.to_string()))?;
        let dec_html = row
            .7
            .map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h))
            .transpose()
            .map_err(|e| DatabaseError::Decryption(e.to_string()))?;

        Ok(nuncio_core::model::Email {
            id: row.0,
            account_id: row.1,
            subject: row.2,
            sender: row.3,
            recipient: row.4,
            received_at: row.5,
            body_plain: dec_plain,
            body_html: dec_html,
            attachments: Vec::new(),
            message_id: row.8,
            content_hash: row.9,
        })
    }

    /// Append a new immutable WORM audit record to the log ledger, signed with the WORM
    /// HMAC key provisioned for this engine from the secret vault.
    ///
    /// The read of the latest sequence/hash and the dependent insert run inside a single
    /// `BEGIN IMMEDIATE` transaction: `BEGIN IMMEDIATE` acquires SQLite's write lock before
    /// any statement runs, so a second concurrent append (from another pooled connection,
    /// since `DatabaseEngine` is shared as an `Arc` across the daemon) blocks until this one
    /// commits rather than reading the same prev-hash and forking the chain.
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

        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(DatabaseError::Query)?;

        let last_row: Option<(i64, String)> = sqlx::query_as(
            "SELECT sequence, record_hmac FROM worm_audit_records ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;

        let (next_seq, prev_hash) = match last_row {
            Some((seq, hash)) => (seq as u64 + 1, hash),
            None => (1, "GENESIS".to_string()),
        };

        let record = nuncio_core::WormAuditRecord::create_signed(
            &self.worm_key[..],
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
        .execute(&mut *tx)
        .await?;

        tx.commit().await.map_err(DatabaseError::Query)?;

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

    /// Keyset-paginated listing of the WORM audit ledger, `sequence ASC`.
    /// `after` is the sequence of the last record of the previous page;
    /// `None` starts from the lowest sequence. Fetches `page_size + 1` rows to
    /// detect a following page and returns the sequence of the last returned
    /// record as the cursor when more remain. Keyset paging on the
    /// monotonically increasing sequence is stable under concurrent appends,
    /// unlike the previous limit/offset paging.
    pub async fn list_worm_audit_records_page(
        &self,
        after: Option<u64>,
        page_size: usize,
    ) -> Result<(Vec<nuncio_core::WormAuditRecord>, Option<u64>), DatabaseError> {
        let fetch = page_size.saturating_add(1);
        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT sequence, timestamp_ns, actor, action, data_hash, previous_block_hash, record_hmac FROM worm_audit_records ",
        );
        if let Some(seq) = after {
            builder.push("WHERE sequence > ");
            builder.push_bind(seq as i64);
        }
        builder.push(" ORDER BY sequence ASC LIMIT ");
        builder.push_bind(fetch as i64);

        let rows = builder
            .build_query_as::<(i64, i64, String, String, String, String, String)>()
            .fetch_all(&self.pool)
            .await?;

        let has_more = rows.len() > page_size;
        let records: Vec<nuncio_core::WormAuditRecord> = rows
            .into_iter()
            .take(page_size)
            .map(|r| nuncio_core::WormAuditRecord {
                sequence: r.0 as u64,
                timestamp_ns: r.1,
                actor: r.2,
                action: r.3,
                data_hash: r.4,
                previous_block_hash: r.5,
                record_hmac: r.6,
            })
            .collect();

        let next = if has_more {
            records.last().map(|r| r.sequence)
        } else {
            None
        };
        Ok((records, next))
    }

    /// Verify the entire WORM cryptographic audit log chain, using the WORM HMAC key
    /// provisioned for this engine.
    pub async fn verify_worm_audit_chain(&self) -> Result<(), DatabaseError> {
        let records = self.list_worm_audit_records(100_000, 0).await?;
        nuncio_core::verify_worm_chain(&records, &self.worm_key[..])
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
        match nuncio_core::verify_worm_chain(&records, &self.worm_key[..]) {
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

        // Record WORM audit log for this data export. If the audit record cannot be
        // persisted, the export must not report success: an export without a WORM
        // record is indistinguishable from an unaudited (and thus untrusted) export.
        self.append_worm_audit_record(
            "system.export",
            "data.export",
            output_path.to_string_lossy().as_bytes(),
        )
        .await?;

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
                keyring_secret_key: "nuncio/acct-contended-1".to_string(),
                sync_interval_secs: 60,
                filters_enabled: false,
                transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                    imap_host: "imap.nuncio.mx".to_string(),
                    imap_port: 993,
                    imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                    smtp_host: "smtp.nuncio.mx".to_string(),
                    smtp_port: 465,
                    smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                }),
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

    /// A hand-edited or corrupted vault entry for the WORM audit key must never be silently
    /// accepted at the wrong length: a 16-byte HMAC key would still "work" mechanically but
    /// would weaken the tamper-evidence the WORM log exists to provide. Construction must fail
    /// closed instead.
    #[tokio::test]
    async fn connect_file_fails_closed_on_wrong_length_worm_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("bad_worm_key.db");
        let secrets = crate::vault::SecretManager::mock();
        secrets
            .set_secret(crate::vault::WORM_KEY_ACCOUNT, &hex::encode([0u8; 16]))
            .expect("seed undersized WORM key material");

        let result = DatabaseEngine::connect_file(&db_path, &secrets).await;
        assert!(
            matches!(result, Err(DatabaseError::KeyProvisioning(_))),
            "a WORM key of the wrong length must fail closed, never be silently accepted as an \
             HMAC signing key: {result:?}"
        );
    }

    /// Mirrors `connect_file_fails_closed_on_wrong_length_worm_key` for the filter execution
    /// ledger's HMAC key.
    #[tokio::test]
    async fn connect_file_fails_closed_on_wrong_length_ledger_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("bad_ledger_key.db");
        let secrets = crate::vault::SecretManager::mock();
        secrets
            .set_secret(crate::vault::LEDGER_KEY_ACCOUNT, &hex::encode([0u8; 16]))
            .expect("seed undersized ledger key material");

        let result = DatabaseEngine::connect_file(&db_path, &secrets).await;
        assert!(
            matches!(result, Err(DatabaseError::KeyProvisioning(_))),
            "a ledger key of the wrong length must fail closed, never be silently accepted as an \
             HMAC signing key: {result:?}"
        );
    }

    #[tokio::test]
    async fn insert_and_query_message() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        sqlx::query(
            "INSERT INTO messages (message_key, account_id, subject, sender, recipient, received_at, body_plain)
             VALUES ('msg-1', 'acct-1', 'Hello', 'alice@nuncio.mx', 'bob@nuncio.mx', 1700000000, 'Hi Bob')",
        )
        .execute(engine.pool())
        .await
        .unwrap();

        let subject: (String,) =
            sqlx::query_as("SELECT subject FROM messages WHERE message_key = 'msg-1'")
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
            subject: "Database Sync Test".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            body_plain: Some("Plaintext content".to_string()),
            body_html: Some("<p>HTML content</p>".to_string()),
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
        };
        let placement = nuncio_core::model::Placement {
            account_id: "acct-1".to_string(),
            folder_id: "INBOX".to_string(),
            uid_validity: "1".to_string(),
            remote_id: "100".to_string(),
            read: false,
        };

        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::Surrogate,
                &placement,
            )
            .await
            .expect("save email succeeds");

        let fetched = engine
            .get_message("msg-db-100")
            .await
            .expect("get message succeeds");
        assert_eq!(fetched.subject, "Database Sync Test");
        assert!(
            !engine.placements_of("msg-db-100").await.unwrap()[0].read,
            "read state now lives on the placement, not the message"
        );

        let msgs = engine
            .list_messages("acct-1", "INBOX", 10)
            .await
            .expect("list messages succeeds");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0.id, "msg-db-100");
        assert_eq!(msgs[0].1.remote_id, "100");

        let folders = engine
            .list_folders("acct-1")
            .await
            .expect("list folders succeeds");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, "INBOX");
        assert_eq!(folders[0].unread_messages, 1);
    }

    /// A `body_plain` column corrupted after a legitimate write (simulating tampering or
    /// bit-rot) must make `get_message` and `list_messages` return
    /// `Err(DatabaseError::Decryption)`, never a silently-empty body -- that would defeat
    /// the AEAD's integrity guarantee exactly where it matters: detecting tampering on read.
    #[tokio::test]
    async fn corrupted_body_ciphertext_fails_closed_on_read() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let email = nuncio_core::model::Email {
            id: "msg-db-corrupt".to_string(),
            account_id: "acct-1".to_string(),
            subject: "Corrupted At Rest".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            body_plain: Some("Sensitive plaintext body".to_string()),
            body_html: None,
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
        };
        let placement = nuncio_core::model::Placement {
            account_id: "acct-1".to_string(),
            folder_id: "INBOX".to_string(),
            uid_validity: "1".to_string(),
            remote_id: "corrupt".to_string(),
            read: false,
        };
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::Surrogate,
                &placement,
            )
            .await
            .expect("save email succeeds");

        // Flip a byte in the stored (still valid-hex) ciphertext to simulate tampering or
        // storage bit-rot.
        let (stored,): (String,) =
            sqlx::query_as("SELECT body_plain FROM messages WHERE message_key = ?")
                .bind("msg-db-corrupt")
                .fetch_one(&engine.pool)
                .await
                .expect("fetch stored ciphertext");
        let mut bytes = hex::decode(&stored).expect("stored ciphertext is valid hex");
        let last_idx = bytes.len() - 1;
        bytes[last_idx] ^= 0xFF;
        let tampered = hex::encode(bytes);

        sqlx::query("UPDATE messages SET body_plain = ? WHERE message_key = ?")
            .bind(&tampered)
            .bind("msg-db-corrupt")
            .execute(&engine.pool)
            .await
            .expect("corrupt stored ciphertext");

        let err = engine
            .get_message("msg-db-corrupt")
            .await
            .expect_err("get_message must fail closed on corrupted ciphertext");
        assert!(matches!(err, DatabaseError::Decryption(_)));

        let err = engine
            .list_messages("acct-1", "INBOX", 10)
            .await
            .expect_err("list_messages must fail closed on corrupted ciphertext");
        assert!(matches!(err, DatabaseError::Decryption(_)));

        // The export path reads the same ciphertext column and must fail closed too --
        // an export that silently dropped an unauthenticated body would write a file
        // the user could not tell apart from a genuinely empty message.
        let err = engine
            .list_messages_for_export(None, None)
            .await
            .expect_err("export must fail closed on corrupted ciphertext");
        assert!(matches!(err, DatabaseError::Decryption(_)));
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

    /// `set_placement_read` flips the persisted `read_flag` in place (both
    /// directions), and reports `sqlx::Error::RowNotFound` for a placement that
    /// was never saved, rather than silently succeeding on a no-op update.
    #[tokio::test]
    async fn set_placement_read_flips_flag_and_reports_not_found_for_missing_placement() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let email = nuncio_core::model::Email {
            id: "msg-mark-1".to_string(),
            account_id: "acct-1".to_string(),
            subject: "Mark Read Test".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            body_plain: None,
            body_html: None,
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
        };
        let placement = nuncio_core::model::Placement {
            account_id: "acct-1".to_string(),
            folder_id: "INBOX".to_string(),
            uid_validity: "1".to_string(),
            remote_id: "mark-1".to_string(),
            read: false,
        };
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::Surrogate,
                &placement,
            )
            .await
            .expect("save email");

        let key = PlacementKey {
            account_id: "acct-1".to_string(),
            folder_id: "INBOX".to_string(),
            uid_validity: "1".to_string(),
            remote_id: "mark-1".to_string(),
        };

        engine
            .set_placement_read(&key, true)
            .await
            .expect("mark read succeeds");
        assert!(engine.placements_of("msg-mark-1").await.unwrap()[0].read);

        engine
            .set_placement_read(&key, false)
            .await
            .expect("mark unread succeeds");
        assert!(!engine.placements_of("msg-mark-1").await.unwrap()[0].read);

        let missing = PlacementKey {
            remote_id: "never-placed".to_string(),
            ..key.clone()
        };
        let err = engine
            .set_placement_read(&missing, true)
            .await
            .expect_err("marking an unknown placement must fail");
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
            keyring_secret_key: "nuncio/acct-test-1".to_string(),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                imap_host: "imap.nuncio.mx".to_string(),
                imap_port: 993,
                imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                smtp_host: "smtp.nuncio.mx".to_string(),
                smtp_port: 465,
                smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            }),
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
        let t = accounts[0].imap_smtp().expect("imap-smtp transport");
        assert_eq!(t.smtp_host, "smtp.nuncio.mx");
        assert_eq!(t.smtp_port, 465);
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
            keyring_secret_key: "nuncio/acct-tls-1".to_string(),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                imap_host: "imap.nuncio.mx".to_string(),
                imap_port: 143,
                imap_tls_mode: nuncio_core::TlsMode::StartTls,
                smtp_host: "smtp.nuncio.mx".to_string(),
                smtp_port: 25,
                smtp_tls_mode: nuncio_core::TlsMode::Plain,
            }),
        };

        engine.save_account(&acct).await.expect("save succeeds");

        let listed = engine.list_accounts().await.expect("list succeeds");
        assert_eq!(listed.len(), 1);
        let lt = listed[0].imap_smtp().expect("imap-smtp transport");
        assert_eq!(lt.imap_tls_mode, nuncio_core::TlsMode::StartTls);
        assert_eq!(lt.smtp_tls_mode, nuncio_core::TlsMode::Plain);

        let fetched = engine
            .get_account("acct-tls-1")
            .await
            .expect("get succeeds")
            .expect("account present");
        let ft = fetched.imap_smtp().expect("imap-smtp transport");
        assert_eq!(ft.imap_tls_mode, nuncio_core::TlsMode::StartTls);
        assert_eq!(ft.smtp_tls_mode, nuncio_core::TlsMode::Plain);
        assert_eq!(fetched, acct);
    }

    /// A CalDAV account's `collection_url` must survive save/reload, and a
    /// mail account's empty `collection_url` must round-trip as empty (not
    /// NULL-mangled), proving the backfill-safe column persists both.
    #[tokio::test]
    async fn caldav_collection_url_survives_save_and_reload() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let caldav = nuncio_core::AccountConfig {
            id: "acct-caldav-store-1".to_string(),
            name: "Work Calendar".to_string(),
            email_address: "cal@nuncio.mx".to_string(),
            keyring_secret_key: "nuncio/acct-caldav-store-1".to_string(),
            sync_interval_secs: 300,
            filters_enabled: false,
            transport: nuncio_core::Transport::Dav(nuncio_core::DavTransport {
                collection_url: "https://dav.example.com/calendars/user/work/".to_string(),
            }),
        };
        engine.save_account(&caldav).await.expect("save succeeds");

        let fetched = engine
            .get_account("acct-caldav-store-1")
            .await
            .expect("get succeeds")
            .expect("account present");
        assert_eq!(fetched, caldav);
        assert_eq!(fetched.protocol(), nuncio_core::AccountProtocol::CalDav);
        assert_eq!(
            fetched.dav_collection_url(),
            Some("https://dav.example.com/calendars/user/work/")
        );
    }

    /// Regression proof that accounts of DIFFERENT transports coexist on disk
    /// and each loads back as its exact original transport variant -- the flat
    /// physical columns must not blur one transport into another on reload.
    #[tokio::test]
    async fn distinct_transport_accounts_round_trip_to_their_own_variant() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let imap = nuncio_core::AccountConfig {
            id: "acct-mix-imap".to_string(),
            name: "IMAP Account".to_string(),
            email_address: "imap@nuncio.mx".to_string(),
            keyring_secret_key: "nuncio/acct-mix-imap".to_string(),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                imap_host: "imap.nuncio.mx".to_string(),
                imap_port: 143,
                imap_tls_mode: nuncio_core::TlsMode::StartTls,
                smtp_host: "smtp.nuncio.mx".to_string(),
                smtp_port: 587,
                smtp_tls_mode: nuncio_core::TlsMode::StartTls,
            }),
        };
        let jmap = nuncio_core::AccountConfig {
            id: "acct-mix-jmap".to_string(),
            name: "JMAP Account".to_string(),
            email_address: "jmap@nuncio.mx".to_string(),
            keyring_secret_key: "nuncio/acct-mix-jmap".to_string(),
            sync_interval_secs: 120,
            filters_enabled: false,
            transport: nuncio_core::Transport::Jmap(nuncio_core::JmapTransport {
                endpoint_host: "jmap.nuncio.mx".to_string(),
            }),
        };
        let dav = nuncio_core::AccountConfig {
            id: "acct-mix-dav".to_string(),
            name: "DAV Account".to_string(),
            email_address: "dav@nuncio.mx".to_string(),
            keyring_secret_key: "nuncio/acct-mix-dav".to_string(),
            sync_interval_secs: 300,
            filters_enabled: false,
            transport: nuncio_core::Transport::Dav(nuncio_core::DavTransport {
                collection_url: "https://dav.example.com/cal/".to_string(),
            }),
        };

        for acct in [&imap, &jmap, &dav] {
            engine.save_account(acct).await.expect("save succeeds");
        }

        let reload = |id: &str| {
            let engine = &engine;
            let id = id.to_string();
            async move {
                engine
                    .get_account(&id)
                    .await
                    .expect("get succeeds")
                    .expect("account present")
            }
        };

        assert_eq!(reload("acct-mix-imap").await, imap);
        assert_eq!(reload("acct-mix-jmap").await, jmap);
        assert_eq!(reload("acct-mix-dav").await, dav);
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
            keyring_secret_key: "nuncio/acct-del-1".to_string(),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                imap_host: "imap.nuncio.mx".to_string(),
                imap_port: 993,
                imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                smtp_host: "smtp.nuncio.mx".to_string(),
                smtp_port: 465,
                smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            }),
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

    /// A rule row that no longer re-parses (e.g. hand-edited NSQL, or a grammar
    /// change that invalidates previously-stored text) must not vanish from
    /// `list_filter_rules` invisibly. This asserts the observable policy chosen
    /// for that case: the rule is excluded from the returned (enforced) set, but
    /// a `WARN`-level log naming the specific rule id is emitted so an operator
    /// can see that a rule stopped being enforced -- silent disappearance would
    /// look identical to "this rule was never uploaded", which is the failure
    /// mode this test guards against.
    #[tokio::test]
    async fn list_filter_rules_warns_on_unparseable_row_instead_of_dropping_silently() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone, Default)]
        struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for CapturedLogs {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("log buffer lock")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> MakeWriter<'a> for CapturedLogs {
            type Writer = CapturedLogs;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        // Build a structurally valid rule (so the store-level id/name/priority
        // bookkeeping is untouched), then corrupt only the persisted NSQL text
        // to simulate a row that no longer parses under the current grammar.
        let nsql = "SELECT * FROM emails WHERE subject CONTAINS 'Spam' ACTION DELETE";
        let mut rule = nuncio_filter::NsqlParser::parse_rule("Spam Filter", 1, nsql).unwrap();
        rule.nsql_text = "THIS IS NOT VALID NSQL !!!".to_string();
        engine
            .save_filter_rule(&rule)
            .await
            .expect("save filter rule");

        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .finish();

        // `set_default` returns a guard that stays active across `.await`
        // points as long as this test stays on the single `#[tokio::test]`
        // current-thread executor, which is the default runtime flavor and
        // exactly what is used here.
        let guard = tracing::subscriber::set_default(subscriber);
        let rules = engine.list_filter_rules().await.expect("list filter rules");
        drop(guard);

        // The unparseable row must not appear in the enforced rule set.
        assert!(rules.is_empty());

        let captured = String::from_utf8(logs.0.lock().expect("log buffer lock").clone())
            .expect("captured log is valid utf8");
        assert!(
            captured.contains(&rule.id),
            "expected the specific rule id to be named in the WARN log, got: {captured}"
        );
        assert!(
            captured.to_ascii_uppercase().contains("WARN"),
            "expected a WARN-level log for the unparseable rule, got: {captured}"
        );
    }

    #[tokio::test]
    async fn test_pending_mutations_and_keyset_chunking() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        for i in 1..=5 {
            let email = nuncio_core::model::Email {
                id: format!("msg-{i:03}"),
                account_id: "acct-1".to_string(),
                subject: format!("Subject {i}"),
                sender: "alice@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: 1700000000 + i,
                body_plain: Some("Hello".to_string()),
                body_html: None,
                attachments: Vec::new(),
                message_id: None,
                content_hash: None,
            };
            let placement = nuncio_core::model::Placement {
                account_id: "acct-1".to_string(),
                folder_id: "inbox".to_string(),
                uid_validity: "1".to_string(),
                remote_id: format!("{i}"),
                read: false,
            };
            engine
                .save_email_at(
                    &email,
                    nuncio_core::model::IdentitySource::Surrogate,
                    &placement,
                )
                .await
                .unwrap();
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

    /// Build a message and its placement exactly as a real sync would when the
    /// server offered no stable identity: an opaque surrogate key hashed over
    /// the addressing coordinates, and a placement carrying those coordinates.
    fn synced_email(
        account_id: &str,
        folder_id: &str,
        uid_validity: &str,
        remote_id: &str,
    ) -> (nuncio_core::model::Email, nuncio_core::model::Placement) {
        (
            nuncio_core::model::Email {
                id: nuncio_core::model::Email::surrogate_id(
                    account_id,
                    folder_id,
                    uid_validity,
                    remote_id,
                ),
                account_id: account_id.to_string(),
                subject: format!("{folder_id}/{remote_id}"),
                sender: "alice@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: 1_700_000_000,
                body_plain: Some("body".to_string()),
                body_html: None,
                attachments: Vec::new(),
                message_id: None,
                content_hash: None,
            },
            nuncio_core::model::Placement {
                account_id: account_id.to_string(),
                folder_id: folder_id.to_string(),
                uid_validity: uid_validity.to_string(),
                remote_id: remote_id.to_string(),
                read: false,
            },
        )
    }

    /// Persist a `synced_email` pair under the surrogate identity tier.
    async fn save_synced(
        engine: &DatabaseEngine,
        pair: &(nuncio_core::model::Email, nuncio_core::model::Placement),
    ) -> SaveOutcome {
        engine
            .save_email_at(
                &pair.0,
                nuncio_core::model::IdentitySource::Surrogate,
                &pair.1,
            )
            .await
            .expect("save synced message")
    }

    /// The C1 regression: the same protocol UID reused across folders or
    /// accounts must persist as DISTINCT rows (never silently overwrite), and a
    /// re-sync of the SAME message must not duplicate the row it already owns.
    #[tokio::test]
    async fn message_identity_prevents_cross_folder_and_cross_account_collision() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        // Same UID (5) in two folders of one account, and again in a second
        // account -- the exact shapes that used to collapse to `imap-uid-5`.
        let inbox = synced_email("acct-1", "INBOX", "42", "5");
        let sent = synced_email("acct-1", "Sent", "7", "5");
        let other_account = synced_email("acct-2", "INBOX", "99", "5");

        // Every surrogate key is distinct, so none can overwrite another.
        assert_ne!(inbox.0.id, sent.0.id);
        assert_ne!(inbox.0.id, other_account.0.id);
        assert_ne!(sent.0.id, other_account.0.id);

        save_synced(&engine, &inbox).await;
        save_synced(&engine, &sent).await;
        save_synced(&engine, &other_account).await;

        // All three coexist as separate messages, each reachable only through
        // its own account's folder -- the second account's INBOX is not folded
        // into the first's.
        assert_eq!(
            engine
                .list_messages("acct-1", "INBOX", 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            engine
                .list_messages("acct-1", "Sent", 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            engine
                .list_messages("acct-2", "INBOX", 10)
                .await
                .unwrap()
                .len(),
            1
        );
        let (messages,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        assert_eq!(messages, 3);

        // Re-syncing the SAME message recomputes the SAME key, so it lands on
        // the row it already owns rather than duplicating it.
        let inbox_resynced = synced_email("acct-1", "INBOX", "42", "5");
        assert_eq!(inbox_resynced.0.id, inbox.0.id);
        let outcome = save_synced(&engine, &inbox_resynced).await;
        assert!(
            !outcome.message_is_new,
            "a re-sync must not create a message"
        );
        assert!(
            !outcome.placement_is_new,
            "nor a second placement in the same mailbox"
        );
        assert_eq!(
            engine
                .list_messages("acct-1", "INBOX", 10)
                .await
                .unwrap()
                .len(),
            1,
            "a re-sync must not duplicate"
        );

        // The placement round-trips the protocol addressing coordinates.
        let placements = engine.placements_of(&inbox.0.id).await.expect("placements");
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].remote_id, "5");
        assert_eq!(placements[0].uid_validity, "42");
        assert_eq!(placements[0].folder_id, "INBOX");
    }

    #[tokio::test]
    async fn opening_a_newer_database_fails_closed_instead_of_writing_the_old_model() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        sqlx::query(&format!(
            "PRAGMA user_version = {}",
            DatabaseEngine::IDENTITY_SCHEMA_VERSION + 1
        ))
        .execute(engine.pool())
        .await
        .unwrap();

        let err = engine
            .reconcile_schema_version()
            .await
            .expect_err("a database from a newer build must not be opened");
        assert!(
            matches!(err, DatabaseError::SchemaTooNew { .. }),
            "got {err:?}"
        );
        // The schema is CREATE TABLE IF NOT EXISTS, so without this an older
        // binary opens a newer database happily and writes the old model into
        // it, leaving two mutually invisible truths in one file.
        assert!(err.to_string().contains("newer than this build supports"));
    }

    #[tokio::test]
    async fn an_identity_schema_bump_rebuilds_cached_mail_and_keeps_configuration() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let account = nuncio_core::AccountConfig {
            id: "acct-reset".to_string(),
            name: "Reset Account".to_string(),
            email_address: "reset@nuncio.mx".to_string(),
            keyring_secret_key: "nuncio/acct-reset".to_string(),
            sync_interval_secs: 60,
            filters_enabled: true,
            transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                imap_host: "imap.nuncio.mx".to_string(),
                imap_port: 993,
                imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                smtp_host: "smtp.nuncio.mx".to_string(),
                smtp_port: 465,
                smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            }),
        };
        engine.save_account(&account).await.expect("save account");
        save_synced(&engine, &synced_email("acct-reset", "INBOX", "42", "7")).await;
        engine
            .save_folder_sync_state("acct-reset", "INBOX", "v1:uidnext:42:8")
            .await
            .expect("save checkpoint");

        // Pretend this store was written by the previous identity generation.
        sqlx::query("PRAGMA user_version = 0")
            .execute(engine.pool())
            .await
            .unwrap();

        engine
            .reconcile_schema_version()
            .await
            .expect("an older store is rebuilt, not rejected");

        // Server-derived state goes: it cannot be reinterpreted under a new
        // identity model, and the server holds the authoritative copy.
        let (message_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        assert_eq!(message_count, 0, "cached mail must be rebuilt");
        assert_eq!(
            engine
                .get_folder_sync_state("acct-reset", "INBOX")
                .await
                .unwrap(),
            None,
            "a checkpoint must not survive an identity change"
        );

        // Everything the user configured survives.
        let accounts = engine.list_accounts().await.expect("list accounts");
        assert_eq!(accounts.len(), 1, "accounts must be preserved");
        assert_eq!(accounts[0].id, "acct-reset");
        // Including which daemon owns filter execution: a rebuild that dropped
        // it would silently switch the owner off, or -- worse, if it defaulted
        // the other way -- switch a bystander on.
        assert!(
            accounts[0].filters_enabled,
            "filter ownership must survive an identity rebuild"
        );

        // The version is recorded, so the next open is a no-op.
        let (version,): (i64,) = sqlx::query_as("PRAGMA user_version")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        assert_eq!(version, DatabaseEngine::IDENTITY_SCHEMA_VERSION);
        engine
            .reconcile_schema_version()
            .await
            .expect("re-running at the current version is a no-op");
    }

    #[tokio::test]
    async fn placements_primary_key_includes_uidvalidity() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        // Same folder, same UID, different UIDVALIDITY: two distinct placements.
        // After a UIDVALIDITY bump the server restarts UIDs at 1, so without
        // uidvalidity in the key the new mail would overwrite the old.
        for validity in ["42", "43"] {
            sqlx::query(
                "INSERT INTO placements (account_id, folder_id, uidvalidity, uid, message_key, read_flag)
                 VALUES (?, ?, ?, ?, ?, 0)",
            )
            .bind("acct-1")
            .bind("INBOX")
            .bind(validity)
            .bind("1")
            .bind(format!("key-{validity}"))
            .execute(engine.pool())
            .await
            .expect("both placements must insert");
        }

        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM placements")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn a_filter_fired_claim_is_won_exactly_once() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        assert!(engine.claim_filter_fire("rule-1", "key-1").await.unwrap());
        assert!(
            !engine.claim_filter_fire("rule-1", "key-1").await.unwrap(),
            "a second claim on the same (rule, message) must lose"
        );
        assert!(
            engine.claim_filter_fire("rule-2", "key-1").await.unwrap(),
            "a different rule still gets its own claim"
        );
    }

    #[tokio::test]
    async fn bumping_the_identity_schema_rebuilds_placements_and_claims_too() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nuncio.db");
        let secrets = crate::vault::SecretManager::mock();

        {
            let engine = DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO placements (account_id, folder_id, uidvalidity, uid, message_key, read_flag)
                 VALUES ('a', 'INBOX', '1', '1', 'k', 0)",
            )
            .execute(engine.pool())
            .await
            .unwrap();
            engine.claim_filter_fire("rule-1", "k").await.unwrap();
            // Pretend this file was written by the previous identity generation.
            sqlx::query("PRAGMA user_version = 1")
                .execute(engine.pool())
                .await
                .unwrap();
            engine.close().await;
        }

        let engine = DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .unwrap();
        let (placements,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM placements")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        let (claims,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM filter_fired")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        assert_eq!(
            placements, 0,
            "placements are derived from the server and must be rebuilt"
        );
        assert_eq!(
            claims, 0,
            "fire-once claims name message keys that no longer resolve"
        );
        engine.close().await;
    }

    /// An `(Email, Placement)` pair for `account_id = "acct-1"`,
    /// `uid_validity = "42"` -- the shape a sync hands the store once identity
    /// and occupancy are separate.
    fn sample_message_and_placement(
        key: &str,
        folder: &str,
        uid: &str,
    ) -> (nuncio_core::model::Email, nuncio_core::model::Placement) {
        (
            nuncio_core::model::Email {
                id: key.into(),
                account_id: "acct-1".into(),
                subject: "Subject".into(),
                sender: "a@nuncio.mx".into(),
                recipient: "b@nuncio.mx".into(),
                received_at: 1_000,
                body_plain: Some("body".into()),
                body_html: None,
                attachments: Vec::new(),
                message_id: None,
                content_hash: None,
            },
            nuncio_core::model::Placement {
                account_id: "acct-1".into(),
                folder_id: folder.into(),
                uid_validity: "42".into(),
                remote_id: uid.into(),
                read: false,
            },
        )
    }

    #[tokio::test]
    async fn a_second_placement_does_not_overwrite_the_stored_body() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let email = nuncio_core::model::Email {
            id: "key-1".into(),
            account_id: "acct-1".into(),
            subject: "Original".into(),
            sender: "a@nuncio.mx".into(),
            recipient: "b@nuncio.mx".into(),
            received_at: 1_000,
            body_plain: Some("the trusted body".into()),
            body_html: None,
            attachments: Vec::new(),
            message_id: Some("a@b.example".into()),
            content_hash: Some("1111".into()),
        };
        let inbox = nuncio_core::model::Placement {
            account_id: "acct-1".into(),
            folder_id: "INBOX".into(),
            uid_validity: "42".into(),
            remote_id: "5".into(),
            read: false,
        };
        let outcome = engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::MessageIdContent,
                &inbox,
            )
            .await
            .unwrap();
        assert!(outcome.message_is_new && outcome.placement_is_new);

        // The same key arriving from another folder, carrying a different body.
        let impostor = nuncio_core::model::Email {
            subject: "Replaced".into(),
            body_plain: Some("attacker body".into()),
            ..email.clone()
        };
        let archive = nuncio_core::model::Placement {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            ..inbox.clone()
        };
        let outcome = engine
            .save_email_at(
                &impostor,
                nuncio_core::model::IdentitySource::MessageIdContent,
                &archive,
            )
            .await
            .unwrap();
        assert!(!outcome.message_is_new, "identity already existed");
        assert!(outcome.placement_is_new, "but the Archive placement is new");

        let stored = engine.get_message("key-1").await.unwrap();
        assert_eq!(stored.subject, "Original", "first write must win");
        assert_eq!(stored.body_plain.as_deref(), Some("the trusted body"));
    }

    #[tokio::test]
    async fn re_placing_the_same_message_leaves_exactly_one_fts_row() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        engine
            .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();
        let archive = nuncio_core::model::Placement {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            ..inbox.clone()
        };
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::EmailId,
                &archive,
            )
            .await
            .unwrap();

        let (rows,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM messages_fts WHERE message_key = 'key-1'")
                .fetch_one(engine.pool())
                .await
                .unwrap();
        assert_eq!(
            rows, 1,
            "the FTS index holds one row per message, not per placement"
        );
    }

    #[tokio::test]
    async fn read_state_is_tracked_per_placement() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        let archive = nuncio_core::model::Placement {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            read: true,
            ..inbox.clone()
        };
        engine
            .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::EmailId,
                &archive,
            )
            .await
            .unwrap();

        let placements = engine.placements_of("key-1").await.unwrap();
        let inbox_read = placements
            .iter()
            .find(|p| p.folder_id == "INBOX")
            .unwrap()
            .read;
        let archive_read = placements
            .iter()
            .find(|p| p.folder_id == "Archive")
            .unwrap()
            .read;
        assert!(!inbox_read);
        assert!(
            archive_read,
            "IMAP \\Seen is per-mailbox, so the flags differ"
        );
    }

    #[tokio::test]
    async fn dropping_one_placement_keeps_the_body_searchable_from_the_other() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        let archive = nuncio_core::model::Placement {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            ..inbox.clone()
        };
        engine
            .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::EmailId,
                &archive,
            )
            .await
            .unwrap();

        let inbox_key = PlacementKey {
            account_id: "acct-1".into(),
            folder_id: "INBOX".into(),
            uid_validity: "42".into(),
            remote_id: "5".into(),
        };
        let outcome = engine.delete_placements(&[inbox_key]).await.unwrap();
        assert_eq!(outcome.placements_removed, 1);
        assert_eq!(
            outcome.messages_reaped, 0,
            "the message still lives in Archive"
        );

        engine
            .get_message("key-1")
            .await
            .expect("the message must survive");
        let (fts,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM messages_fts WHERE message_key = 'key-1'")
                .fetch_one(engine.pool())
                .await
                .unwrap();
        assert_eq!(fts, 1, "search must still find a message that still exists");
    }

    #[tokio::test]
    async fn dropping_the_last_placement_reaps_the_message_and_its_plaintext_index() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        engine
            .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();

        let inbox_key = PlacementKey {
            account_id: "acct-1".into(),
            folder_id: "INBOX".into(),
            uid_validity: "42".into(),
            remote_id: "5".into(),
        };
        let outcome = engine.delete_placements(&[inbox_key]).await.unwrap();
        assert_eq!(outcome.messages_reaped, 1);

        engine
            .get_message("key-1")
            .await
            .expect_err("the message is gone");
        let (fts,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM messages_fts WHERE message_key = 'key-1'")
                .fetch_one(engine.pool())
                .await
                .unwrap();
        assert_eq!(
            fts, 0,
            "an orphaned FTS row would leave the plaintext body readable to anyone \
             with filesystem access after the message itself was deleted"
        );
    }

    #[tokio::test]
    async fn repointing_a_slot_reaps_the_displaced_message_and_its_plaintext_index() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (first, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        engine
            .save_email_at(&first, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();

        // The same mailbox slot re-saved under a different identity. The displaced
        // message never passes through `delete_placements`, so this write is its only
        // chance to be reaped.
        let (second, _) = sample_message_and_placement("key-2", "INBOX", "5");
        engine
            .save_email_at(&second, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();

        engine
            .get_message("key-1")
            .await
            .expect_err("the displaced message has no placements left");
        let (fts,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM messages_fts WHERE message_key = 'key-1'")
                .fetch_one(engine.pool())
                .await
                .unwrap();
        assert_eq!(
            fts, 0,
            "a repoint that strands a message must not leave its decrypted body \
             readable in the FTS index"
        );

        engine
            .get_message("key-2")
            .await
            .expect("the message that took the slot must survive");
        let placements = engine.placements_of("key-2").await.unwrap();
        assert_eq!(placements.len(), 1, "the slot holds exactly one occupant");
    }

    #[tokio::test]
    async fn repointing_a_slot_keeps_a_displaced_message_that_still_lives_elsewhere() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (first, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        let archive = nuncio_core::model::Placement {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            ..inbox.clone()
        };
        engine
            .save_email_at(&first, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();
        engine
            .save_email_at(
                &first,
                nuncio_core::model::IdentitySource::EmailId,
                &archive,
            )
            .await
            .unwrap();

        let (second, _) = sample_message_and_placement("key-2", "INBOX", "5");
        engine
            .save_email_at(&second, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();

        engine
            .get_message("key-1")
            .await
            .expect("the displaced message still lives in Archive");
        let (fts,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM messages_fts WHERE message_key = 'key-1'")
                .fetch_one(engine.pool())
                .await
                .unwrap();
        assert_eq!(fts, 1, "search must still find a message that still exists");
        let placements = engine.placements_of("key-1").await.unwrap();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].folder_id, "Archive");
    }

    #[tokio::test]
    async fn folders_are_derived_from_placements_and_scoped_per_account() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        for (account, folder, uid) in [
            ("acct-1", "INBOX", "1"),
            ("acct-1", "Archive", "2"),
            ("acct-2", "INBOX", "1"),
        ] {
            let (mut email, mut placement) =
                sample_message_and_placement(&format!("key-{account}-{folder}"), folder, uid);
            email.account_id = account.into();
            placement.account_id = account.into();
            engine
                .save_email_at(
                    &email,
                    nuncio_core::model::IdentitySource::EmailId,
                    &placement,
                )
                .await
                .unwrap();
        }

        let folders = engine.list_folders("acct-1").await.unwrap();
        let names: Vec<&str> = folders.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(names, vec!["Archive", "INBOX"]);
        assert!(
            !folders.iter().any(|f| f.total_messages > 1),
            "acct-2's INBOX must not be folded into acct-1's"
        );
    }

    #[tokio::test]
    async fn one_message_in_two_folders_counts_once_in_each() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        let archive = nuncio_core::model::Placement {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            ..inbox.clone()
        };
        engine
            .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::EmailId,
                &archive,
            )
            .await
            .unwrap();

        let folders = engine.list_folders("acct-1").await.unwrap();
        assert_eq!(folders.len(), 2);
        for folder in &folders {
            assert_eq!(
                folder.total_messages, 1,
                "{} should hold the message once",
                folder.id
            );
        }
        let (messages,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        assert_eq!(messages, 1, "but it is still one message");
    }

    #[tokio::test]
    async fn existing_placements_reports_the_occupancy_not_the_message() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        engine
            .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();

        // A *different* message really does occupy Archive. Without this the
        // "Archive is not present" assertion below would hold even if the
        // `IN (...)` filter were dropped entirely, since the table would have
        // no Archive row to over-report in the first place.
        let (other, other_in_archive) = sample_message_and_placement("key-2", "Archive", "77");
        engine
            .save_email_at(
                &other,
                nuncio_core::model::IdentitySource::EmailId,
                &other_in_archive,
            )
            .await
            .unwrap();

        let in_inbox = PlacementKey {
            account_id: "acct-1".into(),
            folder_id: "INBOX".into(),
            uid_validity: "42".into(),
            remote_id: "5".into(),
        };
        let in_archive = PlacementKey {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            ..in_inbox.clone()
        };
        let never_placed = PlacementKey {
            folder_id: "Sent".into(),
            remote_id: "404".into(),
            ..in_inbox.clone()
        };

        let found = engine
            .existing_placements(&[in_inbox.clone(), in_archive.clone(), never_placed.clone()])
            .await
            .unwrap();
        assert!(found.contains(&in_inbox));
        assert!(
            !found.contains(&in_archive),
            "the same message arriving in a new folder is a new placement and must be seen as such"
        );
        assert!(
            !found.contains(&never_placed),
            "over-reporting would make genuinely new mail look already-seen and be skipped"
        );
        assert_eq!(found.len(), 1, "and nothing else may be reported either");

        // The message identity, by contrast, IS already known -- the two
        // questions must not be conflated.
        let known = engine
            .existing_message_ids(&["key-1".to_string(), "key-3-never-saved".to_string()])
            .await
            .unwrap();
        assert!(known.contains("key-1"));
        assert!(
            !known.contains("key-3-never-saved"),
            "a key that was never persisted must never be reported as known"
        );
        assert_eq!(known.len(), 1, "and the query must not echo its own input");
    }

    /// A first sync of a real mailbox hands `existing_placements` the entire
    /// folder in one call. Each key spends four bind parameters, and SQLite
    /// caps a statement at 32766 of them, so an unchunked `IN (...)` list dies
    /// somewhere past 8191 keys -- and dies the same way on every retry, because
    /// the folder sync aborts before writing a checkpoint. 10000 keys is
    /// comfortably over that ceiling, and 6000 of them are really placed, so
    /// this also fails if chunking loses or double-counts a chunk's results
    /// rather than unioning them.
    #[tokio::test]
    async fn existing_placements_handles_a_batch_past_the_bind_parameter_ceiling() {
        const PLACED: usize = 6_000;
        const QUERIED: usize = 10_000;

        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        // Insert the placement rows directly: this test is about the read
        // path's statement size, and 6000 encrypt-and-commit round trips
        // through `save_email_at` would test nothing extra at real cost.
        let mut tx = engine.pool().begin().await.unwrap();
        for uid in 0..PLACED {
            sqlx::query(
                "INSERT INTO placements
                 (account_id, folder_id, uidvalidity, uid, message_key, read_flag)
                 VALUES ('acct-1', 'INBOX', '42', ?, ?, 0)",
            )
            .bind(uid.to_string())
            .bind(format!("key-{uid}"))
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();

        let keys: Vec<PlacementKey> = (0..QUERIED)
            .map(|uid| PlacementKey {
                account_id: "acct-1".into(),
                folder_id: "INBOX".into(),
                uid_validity: "42".into(),
                remote_id: uid.to_string(),
            })
            .collect();

        let found = engine.existing_placements(&keys).await.unwrap();

        assert_eq!(
            found.len(),
            PLACED,
            "every stored placement in the batch must be reported exactly once"
        );
        assert!(
            found.contains(&keys[0]) && found.contains(&keys[PLACED - 1]),
            "hits at both ends of the stored range must survive the split"
        );
        assert!(
            !found.contains(&keys[PLACED]),
            "and a key that was never placed must not be invented by the union"
        );
    }

    /// The same ceiling in the one-parameter-per-key shape. 32766 parameters is
    /// far away here, but the chunk boundary is not, and a batch that straddles
    /// several chunks must still come back as one answer.
    #[tokio::test]
    async fn existing_message_ids_handles_a_batch_spanning_several_chunks() {
        const QUERIED: usize = 10_000;
        // Deliberately placed in different chunks of the split.
        const STORED: [usize; 3] = [0, 5_000, 9_999];

        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        for (nth, index) in STORED.iter().enumerate() {
            let (email, placement) =
                sample_message_and_placement(&format!("key-{index}"), "INBOX", &nth.to_string());
            engine
                .save_email_at(
                    &email,
                    nuncio_core::model::IdentitySource::EmailId,
                    &placement,
                )
                .await
                .unwrap();
        }

        let keys: Vec<String> = (0..QUERIED).map(|n| format!("key-{n}")).collect();
        let found = engine.existing_message_ids(&keys).await.unwrap();

        assert_eq!(found.len(), STORED.len(), "no chunk's result may be lost");
        for index in STORED {
            assert!(found.contains(&format!("key-{index}")));
        }
    }

    /// `save_email_at` reads (the placement existence check) and then writes what
    /// it read, so it must serialize rather than error under concurrency: in WAL
    /// mode a deferred transaction upgrading to a writer aborts with
    /// SQLITE_BUSY_SNAPSHOT, which `busy_timeout` does not retry. Twenty racing
    /// saves of the SAME message and placement must therefore all succeed, and
    /// exactly one of them must claim each of the two "is new" flags -- if two
    /// callers both saw a first arrival, a fire-once filter action would run
    /// twice.
    #[tokio::test]
    async fn concurrent_saves_of_one_placement_serialize_and_elect_one_winner() {
        const CONCURRENT_SAVES: usize = 20;

        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let engine = std::sync::Arc::new(engine);
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");

        let mut handles = Vec::with_capacity(CONCURRENT_SAVES);
        for _ in 0..CONCURRENT_SAVES {
            let engine = std::sync::Arc::clone(&engine);
            let email = email.clone();
            let inbox = inbox.clone();
            handles.push(tokio::spawn(async move {
                engine
                    .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
                    .await
            }));
        }

        let mut new_messages = 0;
        let mut new_placements = 0;
        for handle in handles {
            let outcome = handle.await.expect("save task must not panic").expect(
                "a concurrent save must never fail outright -- it must serialize, not error",
            );
            new_messages += usize::from(outcome.message_is_new);
            new_placements += usize::from(outcome.placement_is_new);
        }
        assert_eq!(new_messages, 1, "exactly one caller created the message");
        assert_eq!(
            new_placements, 1,
            "exactly one caller created the placement"
        );

        let (messages,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        let (fts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages_fts")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        let (placements,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM placements")
            .fetch_one(engine.pool())
            .await
            .unwrap();
        assert_eq!((messages, fts, placements), (1, 1, 1));
    }

    /// Paging a folder must yield every placement in it EXACTLY once, with no
    /// dupes and no gaps -- including the case the keyset exists to survive: one
    /// message occupying a single folder more than once (two `COPY`s of the same
    /// mail land under different UIDs). Every row here also shares one
    /// `received_at`, so the tiebreaker columns carry the whole ordering.
    #[tokio::test]
    async fn list_messages_page_returns_every_placement_once_no_gaps() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let mut expected: Vec<(String, String)> = Vec::new();
        for (key, uid) in [
            ("key-a", "1"),
            ("key-a", "2"),
            ("key-b", "3"),
            ("key-b", "4"),
            ("key-c", "5"),
        ] {
            let (email, placement) = sample_message_and_placement(key, "INBOX", uid);
            engine
                .save_email_at(
                    &email,
                    nuncio_core::model::IdentitySource::EmailId,
                    &placement,
                )
                .await
                .unwrap();
            expected.push((key.to_string(), uid.to_string()));
        }

        let mut seen: Vec<(String, String)> = Vec::new();
        let mut after: Option<MessagePageCursor> = None;
        let mut pages = 0;
        loop {
            let (rows, next) = engine
                .list_messages_page("acct-1", "INBOX", after.clone(), 2)
                .await
                .unwrap();
            pages += 1;
            assert!(rows.len() <= 2, "page must not exceed page_size");
            seen.extend(
                rows.iter()
                    .map(|(e, p)| (e.id.clone(), p.remote_id.clone())),
            );
            match next {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
            assert!(pages < 100, "pagination must terminate");
        }

        let mut deduped = seen.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(deduped.len(), seen.len(), "no placement returned twice");

        expected.sort();
        assert_eq!(
            deduped, expected,
            "every placement in the folder must be reachable across pages"
        );
        assert!(pages >= 3, "5 rows at page_size 2 must span multiple pages");
    }

    #[tokio::test]
    async fn existing_placements_with_empty_input_returns_empty_set_without_querying() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let found = engine.existing_placements(&[]).await.unwrap();

        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn placements_in_folder_reports_only_that_mailbox() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        let archive = nuncio_core::model::Placement {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            ..inbox.clone()
        };
        engine
            .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::EmailId,
                &archive,
            )
            .await
            .unwrap();

        let local = engine
            .placements_in_folder("acct-1", "INBOX")
            .await
            .unwrap();
        assert_eq!(
            local,
            vec![PlacementKey {
                account_id: "acct-1".into(),
                folder_id: "INBOX".into(),
                uid_validity: "42".into(),
                remote_id: "5".into(),
            }],
            "the local half of a UID-set diff is scoped to the folder being synced"
        );
    }

    #[tokio::test]
    async fn marking_read_in_one_folder_leaves_the_other_unread() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
        let archive = nuncio_core::model::Placement {
            folder_id: "Archive".into(),
            remote_id: "9".into(),
            ..inbox.clone()
        };
        engine
            .save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox)
            .await
            .unwrap();
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::EmailId,
                &archive,
            )
            .await
            .unwrap();

        let inbox_key = PlacementKey {
            account_id: "acct-1".into(),
            folder_id: "INBOX".into(),
            uid_validity: "42".into(),
            remote_id: "5".into(),
        };
        engine.set_placement_read(&inbox_key, true).await.unwrap();

        let placements = engine.placements_of("key-1").await.unwrap();
        assert!(
            placements
                .iter()
                .find(|p| p.folder_id == "INBOX")
                .unwrap()
                .read
        );
        assert!(
            !placements
                .iter()
                .find(|p| p.folder_id == "Archive")
                .unwrap()
                .read
        );
    }

    #[tokio::test]
    async fn message_provenance_round_trips_through_every_read_path() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let mut pair = synced_email("acct-1", "INBOX", "42", "7");
        pair.0.message_id = Some("abc123@mail.nuncio.mx".to_string());
        pair.0.content_hash = Some(nuncio_core::model::Email::content_hash_of(b"raw octets"));
        save_synced(&engine, &pair).await;

        let by_key = engine.get_message(&pair.0.id).await.expect("get_message");
        assert_eq!(by_key.message_id, pair.0.message_id);
        assert_eq!(by_key.content_hash, pair.0.content_hash);

        let listed = engine
            .list_messages("acct-1", "INBOX", 10)
            .await
            .expect("list");
        let found = listed
            .iter()
            .find(|(m, _)| m.id == pair.0.id)
            .expect("the saved message is listed");
        assert_eq!(found.0.message_id, pair.0.message_id);
        assert_eq!(found.0.content_hash, pair.0.content_hash);

        let exported = engine
            .list_messages_for_export(None, None)
            .await
            .expect("export");
        let found = exported
            .iter()
            .find(|m| m.id == pair.0.id)
            .expect("the saved message is exportable");
        assert_eq!(found.message_id, pair.0.message_id);
        assert_eq!(found.content_hash, pair.0.content_hash);

        // A message with no Message-ID stays None rather than becoming "".
        let plain = synced_email("acct-1", "INBOX", "42", "8");
        assert_eq!(plain.0.message_id, None);
        save_synced(&engine, &plain).await;
        let fetched = engine.get_message(&plain.0.id).await.expect("get_message");
        assert_eq!(fetched.message_id, None);
        assert_eq!(fetched.content_hash, None);
    }

    /// Save one exportable message into `folder_id` of `account_id`.
    async fn save_export_message(
        engine: &DatabaseEngine,
        id: &str,
        account_id: &str,
        folder_id: &str,
    ) {
        let email = nuncio_core::model::Email {
            id: id.to_string(),
            account_id: account_id.to_string(),
            subject: format!("Subject {id}"),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1_700_000_000,
            body_plain: Some(format!("Body {id}")),
            body_html: None,
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
        };
        let placement = nuncio_core::model::Placement {
            account_id: account_id.to_string(),
            folder_id: folder_id.to_string(),
            uid_validity: "1".to_string(),
            remote_id: id.to_string(),
            read: false,
        };
        engine
            .save_email_at(
                &email,
                nuncio_core::model::IdentitySource::Surrogate,
                &placement,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn existing_message_ids_with_empty_input_returns_empty_set_without_querying() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

        let found = engine.existing_message_ids(&[]).await.unwrap();

        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn list_messages_for_export_with_no_filters_returns_every_message() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        save_export_message(&engine, "msg-1", "acct-a", "inbox").await;
        save_export_message(&engine, "msg-2", "acct-b", "archive").await;

        let all = engine.list_messages_for_export(None, None).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "msg-1");
        assert_eq!(all[1].id, "msg-2");
    }

    #[tokio::test]
    async fn list_messages_for_export_filters_by_account_id() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        save_export_message(&engine, "msg-1", "acct-a", "inbox").await;
        save_export_message(&engine, "msg-2", "acct-b", "inbox").await;

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
        save_export_message(&engine, "msg-1", "acct-a", "inbox").await;
        save_export_message(&engine, "msg-2", "acct-a", "archive").await;

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
        save_export_message(&engine, "msg-1", "acct-a", "inbox").await;
        save_export_message(&engine, "msg-2", "acct-a", "archive").await;
        save_export_message(&engine, "msg-3", "acct-b", "inbox").await;

        let scoped = engine
            .list_messages_for_export(Some("acct-a"), Some("inbox"))
            .await
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "msg-1");
    }

    /// An export whose WORM audit record cannot be persisted must never report success --
    /// an unaudited export would be indistinguishable from a properly-audited one, defeating
    /// the tamper-evidence the WORM log exists to provide. Forces a genuine storage failure
    /// (the pool is closed out from under the engine) rather than a contrived one.
    #[tokio::test]
    async fn export_messages_to_file_fails_when_worm_audit_write_fails() {
        let (engine, dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        save_export_message(&engine, "msg-1", "acct-a", "inbox").await;
        let messages = engine.list_messages_for_export(None, None).await.unwrap();

        engine.close().await;

        let output_path = dir.path().join("export.json");
        let result = engine
            .export_messages_to_file(&messages, nuncio_core::ExportFormat::Json, &output_path)
            .await;

        assert!(
            result.is_err(),
            "an export whose WORM audit record cannot be written must never report success; \
             got: {result:?}"
        );
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

    /// Regression test for a race between the read-latest-hash and the dependent insert in
    /// `append_worm_audit_record`: fires many concurrent appends against the same
    /// `Arc`-shared engine (mirroring how `nunciod` shares `DatabaseEngine`) and requires the
    /// persisted chain to come back as exactly one unbroken sequence, with no forked or
    /// duplicated prev-hash. Without the `BEGIN IMMEDIATE` fix, two tasks can both read the
    /// same latest hash before either inserts, producing two records that both claim the same
    /// `previous_block_hash` -- a fork that `verify_worm_audit_chain` would reject.
    #[tokio::test]
    async fn append_worm_audit_record_is_atomic_under_concurrency() {
        const CONCURRENT_APPENDS: usize = 20;

        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let engine = std::sync::Arc::new(engine);

        let mut handles = Vec::with_capacity(CONCURRENT_APPENDS);
        for i in 0..CONCURRENT_APPENDS {
            let engine = std::sync::Arc::clone(&engine);
            handles.push(tokio::spawn(async move {
                engine
                    .append_worm_audit_record(
                        "system.test",
                        "concurrent.append",
                        format!("payload-{i}").as_bytes(),
                    )
                    .await
            }));
        }
        for handle in handles {
            handle.await.expect("append task must not panic").expect(
                "a concurrent append must never fail outright -- it must serialize, not error",
            );
        }

        let records = engine
            .list_worm_audit_records(CONCURRENT_APPENDS as u32 + 1, 0)
            .await
            .unwrap();
        assert_eq!(
            records.len(),
            CONCURRENT_APPENDS,
            "every concurrent append must persist exactly one record -- no lost writes"
        );

        let mut seen_sequences = std::collections::HashSet::new();
        for (idx, record) in records.iter().enumerate() {
            assert_eq!(
                record.sequence,
                idx as u64 + 1,
                "sequence numbers must be contiguous with no gaps or duplicates"
            );
            assert!(
                seen_sequences.insert(record.sequence),
                "duplicate sequence number {} indicates a forked chain",
                record.sequence
            );
            let expected_prev = if idx == 0 {
                "GENESIS".to_string()
            } else {
                records[idx - 1].record_hmac.clone()
            };
            assert_eq!(
                record.previous_block_hash, expected_prev,
                "record at sequence {} must chain from the immediately preceding record's hmac \
                 -- a mismatch here means two appends forked off the same prev-hash",
                record.sequence
            );
        }

        engine
            .verify_worm_audit_chain()
            .await
            .expect("the full persisted chain must verify as a single valid sequence");
    }

    /// Same regression, for the filter execution log ledger: concurrent
    /// `save_filter_execution_log` calls must serialize into one unbroken `prev_hash`/`hash`
    /// chain rather than forking when two tasks read the same latest hash.
    #[tokio::test]
    async fn save_filter_execution_log_is_atomic_under_concurrency() {
        const CONCURRENT_APPENDS: usize = 20;

        let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let engine = std::sync::Arc::new(engine);

        let mut handles = Vec::with_capacity(CONCURRENT_APPENDS);
        for i in 0..CONCURRENT_APPENDS {
            let engine = std::sync::Arc::clone(&engine);
            handles.push(tokio::spawn(async move {
                engine
                    .save_filter_execution_log(&format!("rule-{i}"), &format!("msg-{i}"), "DELETE")
                    .await
            }));
        }
        for handle in handles {
            handle.await.expect("append task must not panic").expect(
                "a concurrent ledger append must never fail outright -- it must serialize, not error",
            );
        }

        let mut logs = engine
            .list_filter_execution_logs(CONCURRENT_APPENDS + 1)
            .await
            .unwrap();
        // `list_filter_execution_logs` returns newest-first; put it back in chain order.
        logs.reverse();
        assert_eq!(
            logs.len(),
            CONCURRENT_APPENDS,
            "every concurrent ledger append must persist exactly one record -- no lost writes"
        );

        let mut seen_ids = std::collections::HashSet::new();
        for (idx, log) in logs.iter().enumerate() {
            assert!(
                seen_ids.insert(log.id),
                "duplicate row id {} indicates a lost or double-counted write",
                log.id
            );
            let expected_prev = if idx == 0 {
                "GENESIS".to_string()
            } else {
                logs[idx - 1].hash.clone()
            };
            assert_eq!(
                log.prev_hash, expected_prev,
                "log entry {} must chain from the immediately preceding entry's hash -- a \
                 mismatch here means two appends forked off the same prev-hash",
                log.id
            );
        }

        assert!(
            engine.verify_execution_log_chain().await.unwrap(),
            "the full persisted ledger must verify as a single valid chain"
        );
    }

    /// Proves the zeroize mechanism `Zeroizing`'s `Drop` relies on is real, not
    /// decorative: explicitly invoking `Zeroize::zeroize` on key-shaped byte
    /// buffers overwrites every byte with zero. This is checked directly
    /// (without dropping the value) because inspecting freed heap memory would
    /// require `unsafe`, which this workspace forbids.
    #[test]
    fn zeroize_mechanism_actually_wipes_key_bytes() {
        use zeroize::Zeroize;

        let mut fixed = Zeroizing::new([0xABu8; 32]);
        assert!(fixed.iter().all(|&b| b != 0), "fixture must start non-zero");
        fixed.zeroize();
        assert!(
            fixed.iter().all(|&b| b == 0),
            "Zeroize::zeroize must overwrite every byte of a fixed-size key buffer"
        );

        let mut variable = Zeroizing::new(vec![0xCDu8; 32]);
        assert!(
            variable.iter().all(|&b| b != 0),
            "fixture must start non-zero"
        );
        variable.zeroize();
        assert!(
            variable.iter().all(|&b| b == 0),
            "Zeroize::zeroize must overwrite every byte of a heap-allocated key buffer"
        );
    }

    /// `DatabaseEngine`'s manual `Debug` impl must never print the raw
    /// storage/WORM/ledger key bytes it now holds in `Zeroizing` wrappers,
    /// even after they were introduced.
    #[tokio::test]
    async fn debug_impl_never_prints_raw_key_bytes() {
        let (engine, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let debug_output = format!("{engine:?}");
        assert!(
            !debug_output.contains("storage_key")
                && !debug_output.contains("worm_key")
                && !debug_output.contains("ledger_key"),
            "Debug output must never expose key field names/values: {debug_output}"
        );
    }
}
