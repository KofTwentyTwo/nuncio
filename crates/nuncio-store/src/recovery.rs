//! Database corruption detection, backup isolation, and stream recovery salvage engine.

use crate::db::{DatabaseEngine, DatabaseError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Raw row shape for an account configuration record fetched from SQLite during salvage.
/// The trailing two `String` columns are the raw `imap_tls_mode`/`smtp_tls_mode` text values
/// (e.g. `"start_tls"`) -- carried through as the literal on-disk token rather than already
/// parsed to [`nuncio_core::TlsMode`], so [`SqliteRecoveryEngine::salvage_accounts`] can log
/// precisely which raw value it could not recognize, if any, instead of a decode failure
/// having already been silently absorbed by this row type.
type SalvagedAccountRow = (
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
);

/// Summary report of database self-healing recovery output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySummary {
    /// Absolute path to preserved forensic backup file.
    pub backup_path: PathBuf,
    /// Number of salvaged account configurations.
    pub salvaged_accounts_count: usize,
    /// Number of salvaged NSQL filter rules.
    pub salvaged_rules_count: usize,
    /// Number of salvaged filter condition clauses.
    pub salvaged_conditions_count: usize,
    /// Number of salvaged filter action specifications.
    pub salvaged_actions_count: usize,
    /// Number of salvaged WORM audit ledger records.
    pub salvaged_audit_records_count: usize,
    /// Number of salvaged filter execution log entries.
    pub salvaged_execution_logs_count: usize,
    /// Indicates whether remote protocol resynchronization was initiated.
    pub resync_triggered: bool,
}

/// Helper identifying SQLite corruption error codes, as distinct from transient/operational
/// errors (busy, locked, timed out, momentarily unable to open).
///
/// This classification is data-safety-critical: only errors classified `true` here are
/// permitted to trigger backup isolation + destructive salvage of the live database (which
/// deletes the original file, keeping only what could be salvaged). A false positive here
/// would destroy a perfectly healthy database on nothing more than a transient hiccup, so the
/// classifier is deliberately an *allowlist* of unambiguous corruption signatures rather than a
/// denylist that grows to catch "everything that isn't obviously fine".
pub fn is_sqlite_corruption_error(err: &sqlx::Error) -> bool {
    // A structured SQLite result code, when sqlx can supply one, is authoritative. sqlx-sqlite
    // reports the *extended* result code (see `sqlite3_extended_errcode`), so the primary code
    // is recovered via `code & 0xff` before comparing -- this still matches corruption
    // sub-variants such as `SQLITE_CORRUPT_VTAB`/`SQLITE_CORRUPT_INDEX` while continuing to
    // exclude every non-corruption code (SQLITE_BUSY = 5, SQLITE_LOCKED = 6, SQLITE_IOERR = 10,
    // SQLITE_CANTOPEN = 14, and their extended sub-variants), which describe operational
    // conditions -- another connection holding a lock, a momentary disk I/O hiccup, a file the
    // OS would not open just now -- that clear up on retry and must NEVER be treated as
    // corruption.
    if let sqlx::Error::Database(db_err) = err {
        return db_err
            .code()
            .and_then(|code| code.parse::<i32>().ok())
            .map(|code| {
                const SQLITE_CORRUPT: i32 = 11;
                const SQLITE_NOTADB: i32 = 26;
                let primary = code & 0xff;
                primary == SQLITE_CORRUPT || primary == SQLITE_NOTADB
            })
            .unwrap_or(false);
    }

    // Errors without a structured database code (e.g. a connection-level failure raised before
    // sqlx has parsed a SQLite result code) are classified from message text, restricted to the
    // same narrow, unambiguous corruption signatures used above.
    let msg = err.to_string().to_lowercase();
    msg.contains("database disk image is malformed") || msg.contains("file is not a database")
}

/// SQLite's fixed 16-byte file header magic string (always the first 16 bytes of a valid
/// SQLite database file).
const SQLITE_HEADER_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Directly inspect the raw SQLite header magic bytes on disk, bypassing any SQLite
/// connection, connection pool, or pager cache entirely.
///
/// This exists because corruption detection routed entirely through a SQLite connection
/// (`connect` + `PRAGMA quick_check`) can race a WAL checkpoint or a connection-close from a
/// prior engine instance still completing in the background: depending on timing, a freshly
/// opened connection can occasionally fail to observe a header that was corrupted moments
/// earlier. A plain `std::fs` read of the header bytes has no such race -- it is a single
/// synchronous read of exactly what is on disk right now, independent of any connection state
/// -- so it gives a deterministic, reproducible answer for the header-corruption case
/// specifically, and is used as an authoritative pre-flight signal in
/// [`crate::db::DatabaseEngine::open_with_backup_dir`].
///
/// Returns `true` only when the file exists, is at least header-sized, and its first 16 bytes
/// do not match SQLite's fixed magic string -- i.e. never for a missing file or a freshly
/// created (not yet initialized) empty database file, only for a genuinely unreadable header.
pub fn has_corrupted_sqlite_header(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() >= SQLITE_HEADER_MAGIC.len() => {
            bytes[0..SQLITE_HEADER_MAGIC.len()] != SQLITE_HEADER_MAGIC[..]
        }
        _ => false,
    }
}

/// Manager isolating damaged database files into forensic backups.
#[derive(Debug, Clone)]
pub struct CorruptedBackupManager;

impl CorruptedBackupManager {
    /// Default root backup directory (`~/.nuncio/corrupted_backups`).
    pub fn default_backup_dir() -> PathBuf {
        if let Ok(home_str) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            PathBuf::from(home_str)
                .join(".nuncio")
                .join("corrupted_backups")
        } else {
            PathBuf::from(".nuncio").join("corrupted_backups")
        }
    }

    /// Isolate damaged database file and companion WAL/SHM files to default backup location.
    pub fn backup_corrupted_db(db_path: &Path) -> Result<PathBuf, DatabaseError> {
        let backup_dir = Self::default_backup_dir();
        Self::backup_corrupted_db_to(db_path, &backup_dir)
    }

    /// Isolate damaged database file and companion WAL/SHM files to a target backup directory.
    pub fn backup_corrupted_db_to(
        db_path: &Path,
        backup_dir: &Path,
    ) -> Result<PathBuf, DatabaseError> {
        std::fs::create_dir_all(backup_dir).map_err(|e| {
            DatabaseError::RecoveryFailed(format!("failed to create backup dir: {e}"))
        })?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let filename = format!("nuncio_corrupted_{}.db", timestamp);
        let target_path = backup_dir.join(&filename);

        if db_path.exists() {
            std::fs::copy(db_path, &target_path).map_err(|e| {
                DatabaseError::RecoveryFailed(format!("failed to copy db file: {e}"))
            })?;
        }

        // Copy companion WAL file if present
        let wal_path = PathBuf::from(format!("{}-wal", db_path.to_string_lossy()));
        if wal_path.exists() {
            let target_wal = PathBuf::from(format!("{}-wal", target_path.to_string_lossy()));
            let _ = std::fs::copy(wal_path, target_wal);
        }

        // Copy companion SHM file if present
        let shm_path = PathBuf::from(format!("{}-shm", db_path.to_string_lossy()));
        if shm_path.exists() {
            let target_shm = PathBuf::from(format!("{}-shm", target_path.to_string_lossy()));
            let _ = std::fs::copy(shm_path, target_shm);
        }

        info!(
            "Corrupted database isolated to forensic backup: {}",
            target_path.display()
        );
        Ok(target_path)
    }
}

/// Stream salvage recovery engine extracting valid metadata tables into a fresh SQLite file.
pub struct SqliteRecoveryEngine;

impl SqliteRecoveryEngine {
    /// Salvage readable accounts and filter rules from damaged database or backup file,
    /// re-creating a fresh SQLite file at `target_db_path`. Cryptographic key material for
    /// the freshly re-created engine is provisioned from `secrets`.
    ///
    /// # Precondition
    ///
    /// The caller MUST have fully closed (via [`DatabaseEngine::close`], which awaits every
    /// pooled connection's teardown) any engine still holding `target_db_path` before calling
    /// this. Salvage unlinks `target_db_path` and its `-wal`/`-shm` companions and then creates
    /// a brand-new database at that same path; a connection left open over the old file is torn
    /// down asynchronously, and SQLite's close-time WAL handling operates on `-wal`/`-shm` *by
    /// path*. A teardown landing after the unlink therefore collides with the fresh database's
    /// own companions -- leaving the new, empty main file paired with a `-wal` describing the
    /// old one, whose replay reads past end-of-file and fails with `SQLITE_IOERR_SHORT_READ`.
    /// `DatabaseEngine::close_and_salvage` is the in-tree caller that honours this.
    pub async fn salvage(
        corrupted_db_path: &Path,
        target_db_path: &Path,
        backup_dir: &Path,
        secrets: &crate::vault::SecretManager,
    ) -> Result<RecoverySummary, DatabaseError> {
        // Step 1: Preserve raw corrupted database to forensic backup directory
        let backup_path =
            CorruptedBackupManager::backup_corrupted_db_to(corrupted_db_path, backup_dir)?;

        // Step 2: Attempt reading valid records from preserved backup file
        let backup_url = format!("sqlite://{}", backup_path.to_string_lossy());
        let (
            salvaged_accounts,
            salvaged_rules,
            salvaged_conditions,
            salvaged_actions,
            salvaged_audit_records,
            salvaged_execution_logs,
        ) = if let Ok(pool) = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&backup_url)
            .await
        {
            let accounts = Self::salvage_accounts(&pool).await;
            let (rules, conditions, actions) = Self::salvage_filter_tables(&pool).await;
            let audit_records = Self::salvage_worm_audit_records(&pool).await;
            let execution_logs = Self::salvage_filter_execution_logs(&pool).await;
            pool.close().await;
            (
                accounts,
                rules,
                conditions,
                actions,
                audit_records,
                execution_logs,
            )
        } else {
            warn!("Failed to open backup connection for salvage; proceeding with clean database reset.");
            (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        };

        // Step 3: Remove or truncate damaged database files at target_db_path
        let _ = std::fs::remove_file(target_db_path);
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(target_db_path);
        let _ = std::fs::remove_file(format!("{}-wal", target_db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", target_db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-wal", target_db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", target_db_path.to_string_lossy()));

        // Step 4: Create fresh SQLite database file and apply migrations
        let fresh_engine = DatabaseEngine::connect_file(target_db_path, secrets).await?;

        // Step 5: Restore salvaged records into new database
        let mut restored_accounts_count = 0;
        for acct in &salvaged_accounts {
            if fresh_engine.save_account(acct).await.is_ok() {
                restored_accounts_count += 1;
            }
        }

        // Column order/names below MUST match the live `filter_rules` / `filter_conditions` /
        // `filter_actions` schema created in `DatabaseEngine::migrate` -- including every
        // NOT NULL column -- or every insert silently fails (via `res.is_ok()`) and salvage
        // reports 0 restored rows despite having read valid data from the backup.
        let mut restored_rules_count = 0;
        for (id, name, priority, enabled, nsql_text, created_at, updated_at) in &salvaged_rules {
            let res = sqlx::query(
                "INSERT INTO filter_rules (id, name, priority, enabled, nsql_text, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(id)
            .bind(name)
            .bind(priority)
            .bind(enabled)
            .bind(nsql_text)
            .bind(created_at)
            .bind(updated_at)
            .execute(fresh_engine.pool())
            .await;
            if res.is_ok() {
                restored_rules_count += 1;
            }
        }

        let mut restored_conditions_count = 0;
        for (id, rule_id, field, operator, value, logical_op) in &salvaged_conditions {
            let res = sqlx::query(
                "INSERT INTO filter_conditions (id, rule_id, field, operator, value, logical_op) VALUES (?, ?, ?, ?, ?, ?)"
            )
            .bind(id)
            .bind(rule_id)
            .bind(field)
            .bind(operator)
            .bind(value)
            .bind(logical_op)
            .execute(fresh_engine.pool())
            .await;
            if res.is_ok() {
                restored_conditions_count += 1;
            }
        }

        let mut restored_actions_count = 0;
        for (id, rule_id, action_type, target) in &salvaged_actions {
            let res = sqlx::query(
                "INSERT INTO filter_actions (id, rule_id, action_type, target) VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(rule_id)
            .bind(action_type)
            .bind(target)
            .execute(fresh_engine.pool())
            .await;
            if res.is_ok() {
                restored_actions_count += 1;
            }
        }

        // Restore the WORM audit ledger and filter execution log ledger verbatim, including
        // their hash-chain columns (`record_hmac`/`hash`, `previous_block_hash`/`prev_hash`).
        // These are re-signed with the same HMAC key material the corrupted database used
        // (both engines are provisioned from the same `secrets` vault), so a chain that
        // verified before corruption still verifies after salvage -- salvage restores the
        // ledger's tamper-evidence, it does not merely copy rows that happen to look right.
        let mut restored_audit_records_count = 0;
        for (sequence, timestamp_ns, actor, action, data_hash, previous_block_hash, record_hmac) in
            &salvaged_audit_records
        {
            let res = sqlx::query(
                "INSERT INTO worm_audit_records (sequence, timestamp_ns, actor, action, data_hash, previous_block_hash, record_hmac) VALUES (?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(sequence)
            .bind(timestamp_ns)
            .bind(actor)
            .bind(action)
            .bind(data_hash)
            .bind(previous_block_hash)
            .bind(record_hmac)
            .execute(fresh_engine.pool())
            .await;
            if res.is_ok() {
                restored_audit_records_count += 1;
            }
        }

        let mut restored_execution_logs_count = 0;
        for (id, rule_id, message_id, action_taken, matched_at, prev_hash, hash) in
            &salvaged_execution_logs
        {
            let res = sqlx::query(
                "INSERT INTO filter_execution_logs (id, rule_id, message_id, action_taken, matched_at, prev_hash, hash) VALUES (?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(id)
            .bind(rule_id)
            .bind(message_id)
            .bind(action_taken)
            .bind(matched_at)
            .bind(prev_hash)
            .bind(hash)
            .execute(fresh_engine.pool())
            .await;
            if res.is_ok() {
                restored_execution_logs_count += 1;
            }
        }

        let summary = RecoverySummary {
            backup_path,
            salvaged_accounts_count: restored_accounts_count,
            salvaged_rules_count: restored_rules_count,
            salvaged_conditions_count: restored_conditions_count,
            salvaged_actions_count: restored_actions_count,
            salvaged_audit_records_count: restored_audit_records_count,
            salvaged_execution_logs_count: restored_execution_logs_count,
            resync_triggered: true,
        };

        info!("SqliteRecoveryEngine salvage finished: {:?}", summary);
        Ok(summary)
    }

    async fn salvage_accounts(pool: &sqlx::SqlitePool) -> Vec<nuncio_core::AccountConfig> {
        let rows: Result<Vec<SalvagedAccountRow>, _> = sqlx::query_as(
            "SELECT id, name, email_address, protocol, server_host, server_port, keyring_secret_key, sync_interval_secs, smtp_host, smtp_port, imap_tls_mode, smtp_tls_mode, collection_url FROM accounts"
        )
        .fetch_all(pool)
        .await;

        match rows {
            Ok(vec) => vec
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
                        imap_tls_mode_raw,
                        smtp_tls_mode_raw,
                        collection_url,
                    )| {
                        let protocol = serde_json::from_str(&protocol_str)
                            .unwrap_or(nuncio_core::AccountProtocol::ImapSmtp);
                        // Backfill-safe fallback: see
                        // `DatabaseEngine::list_accounts` for the matching
                        // rationale -- a salvaged row written before
                        // `smtp_host`/`smtp_port` existed falls back to the
                        // IMAP/JMAP endpoint rather than salvaging an
                        // incomplete config.
                        let resolved_smtp_host = smtp_host.unwrap_or_else(|| server_host.clone());
                        let resolved_smtp_port =
                            smtp_port.map(|p| p as u16).unwrap_or(server_port as u16);
                        let imap_tls_mode =
                            Self::parse_salvaged_tls_mode(&id, "imap_tls_mode", &imap_tls_mode_raw);
                        let smtp_tls_mode =
                            Self::parse_salvaged_tls_mode(&id, "smtp_tls_mode", &smtp_tls_mode_raw);
                        // Reconstruct the transport variant from the salvaged
                        // `protocol` discriminator, mirroring
                        // `DatabaseEngine::list_accounts`.
                        let transport = match protocol {
                            nuncio_core::AccountProtocol::ImapSmtp => {
                                nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                                    imap_host: server_host,
                                    imap_port: server_port as u16,
                                    imap_tls_mode,
                                    smtp_host: resolved_smtp_host,
                                    smtp_port: resolved_smtp_port,
                                    smtp_tls_mode,
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
                            nuncio_core::AccountProtocol::CardDav => {
                                nuncio_core::Transport::CardDav(nuncio_core::DavTransport {
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
                            // Salvage deliberately does not recover this flag.
                            // The column may be among what the corruption took,
                            // and re-enabling filter execution on a guess would
                            // start forwarding mail from a daemon the user never
                            // chose as the owner. Off is recoverable by one
                            // setting; wrongly on is not recoverable at all.
                            filters_enabled: false,
                            transport,
                        }
                    },
                )
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Parse a salvaged account's raw `imap_tls_mode`/`smtp_tls_mode` column text back into
    /// [`nuncio_core::TlsMode`], preserving the account's real configured security posture
    /// (implicit TLS, STARTTLS, or plaintext) rather than hardcoding every salvaged account
    /// back to implicit TLS regardless of how it was actually configured -- silently
    /// upgrading a STARTTLS account or downgrading a plaintext one to "implicit TLS" on
    /// recovery would misrepresent the account's security posture to the operator.
    ///
    /// A raw value that does not match one of the three known on-disk tokens (a row from a
    /// build that predates this column, or a distinct new corruption) cannot be honestly
    /// recovered. Rather than silently guessing at one of the other two modes, this WARN-logs
    /// the account id and the unrecognized raw value, and falls back to the safest mode
    /// (implicit TLS) -- the operator can see in the daemon's logs exactly which account
    /// needs its TLS mode re-confirmed, instead of a quiet, invisible substitution.
    fn parse_salvaged_tls_mode(account_id: &str, column: &str, raw: &str) -> nuncio_core::TlsMode {
        match raw {
            "implicit_tls" => nuncio_core::TlsMode::ImplicitTls,
            "start_tls" => nuncio_core::TlsMode::StartTls,
            "plain" => nuncio_core::TlsMode::Plain,
            other => {
                warn!(
                    "Salvage could not recognize account {account_id}'s {column} value {other:?}; \
                     defaulting to ImplicitTls and flagging for operator review rather than \
                     silently guessing its prior TLS mode."
                );
                nuncio_core::TlsMode::ImplicitTls
            }
        }
    }

    /// Read back valid rows from `worm_audit_records` in the backup connection, preserving
    /// every column including the hash-chain fields (`previous_block_hash`/`record_hmac`) so
    /// the restored ledger's tamper-evidence carries through salvage rather than being
    /// silently dropped.
    async fn salvage_worm_audit_records(
        pool: &sqlx::SqlitePool,
    ) -> Vec<(i64, i64, String, String, String, String, String)> {
        sqlx::query_as::<_, (i64, i64, String, String, String, String, String)>(
            "SELECT sequence, timestamp_ns, actor, action, data_hash, previous_block_hash, record_hmac FROM worm_audit_records ORDER BY sequence ASC"
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
    }

    /// Read back valid rows from `filter_execution_logs` in the backup connection, preserving
    /// every column including the hash-chain fields (`prev_hash`/`hash`) so the restored
    /// ledger's tamper-evidence carries through salvage rather than being silently dropped.
    async fn salvage_filter_execution_logs(
        pool: &sqlx::SqlitePool,
    ) -> Vec<(i64, String, String, String, i64, String, String)> {
        sqlx::query_as::<_, (i64, String, String, String, i64, String, String)>(
            "SELECT id, rule_id, message_id, action_taken, matched_at, prev_hash, hash FROM filter_execution_logs ORDER BY id ASC"
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
    }

    /// Read back valid rows from `filter_rules` / `filter_conditions` / `filter_actions` in the
    /// backup connection, using column names/order matching the live schema (see
    /// `DatabaseEngine::migrate`). `filter_conditions.value`/`filter_actions.target` are the
    /// only nullable columns in that pair of tables that this reads.
    #[allow(clippy::type_complexity)]
    async fn salvage_filter_tables(
        pool: &sqlx::SqlitePool,
    ) -> (
        Vec<(String, String, i64, i64, String, i64, i64)>,
        Vec<(String, String, String, String, String, String)>,
        Vec<(String, String, String, Option<String>)>,
    ) {
        let rules = sqlx::query_as::<_, (String, String, i64, i64, String, i64, i64)>(
            "SELECT id, name, priority, enabled, nsql_text, created_at, updated_at FROM filter_rules"
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let conditions = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT id, rule_id, field, operator, value, logical_op FROM filter_conditions",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let actions = sqlx::query_as::<_, (String, String, String, Option<String>)>(
            "SELECT id, rule_id, action_type, target FROM filter_actions",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        (rules, conditions, actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::AccountConfig;
    use tempfile::tempdir;

    #[test]
    fn test_is_sqlite_corruption_error() {
        let err = sqlx::Error::PoolTimedOut;
        assert!(!is_sqlite_corruption_error(&err));
    }

    #[test]
    fn has_corrupted_sqlite_header_is_false_for_missing_or_fresh_files() {
        let dir = tempdir().expect("tempdir");
        // A path that has never been created at all.
        assert!(!has_corrupted_sqlite_header(
            &dir.path().join("does_not_exist.db")
        ));

        // A freshly created, empty (not-yet-initialized) file: shorter than the header, must
        // never be reported as "corrupted".
        let empty_path = dir.path().join("empty.db");
        std::fs::write(&empty_path, []).unwrap();
        assert!(!has_corrupted_sqlite_header(&empty_path));
    }

    #[test]
    fn has_corrupted_sqlite_header_is_true_only_for_a_bad_magic_string() {
        let dir = tempdir().expect("tempdir");

        let valid_path = dir.path().join("valid_header.db");
        let mut valid_bytes = vec![0u8; 4096];
        valid_bytes[0..16].copy_from_slice(SQLITE_HEADER_MAGIC);
        std::fs::write(&valid_path, &valid_bytes).unwrap();
        assert!(!has_corrupted_sqlite_header(&valid_path));

        let bad_path = dir.path().join("bad_header.db");
        let mut bad_bytes = vec![0u8; 4096];
        bad_bytes[0..16].copy_from_slice(b"CORRUPTED_NOISE_");
        std::fs::write(&bad_path, &bad_bytes).unwrap();
        assert!(has_corrupted_sqlite_header(&bad_path));
    }

    #[tokio::test]
    async fn test_database_header_corruption_stage_1_detection() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("corrupt_test.db");
        let secrets = crate::vault::SecretManager::mock();

        // Step A: Create and populate valid database
        {
            let engine = DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .unwrap();
            let acct = AccountConfig {
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
            engine.save_account(&acct).await.unwrap();
            assert!(engine.check_integrity().await.unwrap());
            engine.close().await;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        // Step B: Corrupt file header bytes on disk and overwrite WAL index with noise
        let mut file_bytes = std::fs::read(&db_path).unwrap();
        if file_bytes.len() < 4096 {
            file_bytes.resize(8192, 0xAB);
        }
        file_bytes[0..16].copy_from_slice(b"CORRUPTED_NOISE_");
        std::fs::write(&db_path, file_bytes).unwrap();
        let wal_p = PathBuf::from(format!("{}-wal", db_path.to_string_lossy()));
        let shm_p = PathBuf::from(format!("{}-shm", db_path.to_string_lossy()));
        let _ = std::fs::write(&wal_p, vec![0xFF; 4096]);
        let _ = std::fs::write(&shm_p, vec![0xFF; 4096]);
        let _ = std::fs::remove_file(&wal_p);
        let _ = std::fs::remove_file(&shm_p);

        // Step C: Verify Stage 1 integrity check / open handles corruption
        let backup_dir = dir.path().join("backups");
        let (recovered_engine, summary) =
            DatabaseEngine::open_with_backup_dir(&db_path, &backup_dir, &secrets)
                .await
                .expect("open_with_backup_dir auto-recovers");

        assert!(summary.is_some());
        let sum = summary.unwrap();
        assert!(sum.backup_path.exists());
        assert!(recovered_engine.check_integrity().await.unwrap());
    }

    #[tokio::test]
    async fn test_stage_2_backup_creation_and_stage_3_table_salvage() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("salvage_test.db");
        let backup_dir = dir.path().join("corrupted_backups");
        let secrets = crate::vault::SecretManager::mock();

        // Step 1: Create DB with account & a real NSQL filter rule (exercising the
        // `filter_rules` schema the salvage restore INSERT runs against).
        {
            let engine = DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .unwrap();
            let acct = AccountConfig {
                id: "acct-salvage-1".to_string(),
                name: "Salvage Account".to_string(),
                email_address: "salvage@nuncio.mx".to_string(),
                keyring_secret_key: "nuncio/acct-salvage-1".to_string(),
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

            let nsql = "SELECT * FROM emails WHERE subject CONTAINS 'Spam' ACTION DELETE";
            let rule = nuncio_filter::NsqlParser::parse_rule("Spam Filter", 1, nsql).unwrap();
            engine.save_filter_rule(&rule).await.unwrap();

            // Closing before salvage is the documented precondition of
            // `SqliteRecoveryEngine::salvage`, and it is what the production caller
            // (`DatabaseEngine::close_and_salvage`) does. Merely dropping the engine is not
            // equivalent: the pool's teardown is deferred onto the runtime, so the old
            // connection's close-time WAL handling can land *after* salvage has unlinked
            // `-wal`/`-shm` and created a fresh database at this same path -- pairing the new,
            // empty main file with a `-wal` describing the old one and failing the subsequent
            // page read with `SQLITE_IOERR_SHORT_READ`. `close()` awaits that teardown, so the
            // window does not exist at all rather than merely being narrow.
            engine.close().await;
        }

        // Step 2: Perform salvage recovery
        let summary = SqliteRecoveryEngine::salvage(&db_path, &db_path, &backup_dir, &secrets)
            .await
            .expect("salvage succeeds");

        assert_eq!(summary.salvaged_accounts_count, 1);
        assert_eq!(
            summary.salvaged_rules_count, 1,
            "salvage must restore the filter rule -- a 0 count here means the restore INSERT \
             is silently failing against a schema mismatch between it and the live table"
        );
        assert!(summary.backup_path.exists());

        // Step 3: Verify fresh DB contains the salvaged account AND filter rule.
        let fresh = DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .unwrap();
        let accounts = fresh.list_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acct-salvage-1");

        let rules = fresh.list_filter_rules().await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "Spam Filter");
    }

    #[tokio::test]
    async fn salvage_preserves_worm_audit_and_filter_execution_log_chains() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("ledger_salvage_test.db");
        let backup_dir = dir.path().join("corrupted_backups");
        let secrets = crate::vault::SecretManager::mock();

        // Step 1: Seed a WORM audit record and a filter execution log entry, each of which
        // is only valid because it is cryptographically hash-chained to its predecessor.
        {
            let engine = DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .unwrap();
            engine
                .append_worm_audit_record("system.test", "account.created", b"payload-1")
                .await
                .unwrap();
            engine
                .append_worm_audit_record("system.test", "account.updated", b"payload-2")
                .await
                .unwrap();
            engine
                .save_filter_execution_log("rule-1", "msg-1", "delete")
                .await
                .unwrap();
            engine
                .save_filter_execution_log("rule-1", "msg-2", "archive")
                .await
                .unwrap();

            assert!(engine.verify_worm_audit_chain().await.is_ok());
            assert!(engine.verify_execution_log_chain().await.unwrap());
        }

        // Step 2: Perform salvage recovery, reusing the same secrets vault -- the WORM/ledger
        // HMAC keys are provisioned from `secrets`, so the restored ledgers verify against
        // the same key material the corrupted database used, exactly as in the real recovery
        // flow (the daemon always re-provisions salvage from the account's existing vault).
        let summary = SqliteRecoveryEngine::salvage(&db_path, &db_path, &backup_dir, &secrets)
            .await
            .expect("salvage succeeds");

        assert_eq!(
            summary.salvaged_audit_records_count, 2,
            "salvage must carry worm_audit_records through recovery instead of silently \
             dropping the tamper-evident audit ledger"
        );
        assert_eq!(
            summary.salvaged_execution_logs_count, 2,
            "salvage must carry filter_execution_logs through recovery instead of silently \
             dropping the filter ledger"
        );

        // Step 3: The restored ledgers must still be present AND still pass hash-chain
        // verification -- proving the chain-linking columns (`previous_block_hash`/
        // `record_hmac`, `prev_hash`/`hash`) were preserved verbatim, not just the row data.
        let fresh = DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .unwrap();

        let audit_records = fresh.list_worm_audit_records(100, 0).await.unwrap();
        assert_eq!(audit_records.len(), 2);
        assert!(
            fresh.verify_worm_audit_chain().await.is_ok(),
            "restored WORM audit chain must still verify after salvage"
        );

        let execution_logs = fresh.list_filter_execution_logs(100).await.unwrap();
        assert_eq!(execution_logs.len(), 2);
        assert!(
            fresh.verify_execution_log_chain().await.unwrap(),
            "restored filter execution log chain must still verify after salvage"
        );
    }

    #[tokio::test]
    async fn salvage_preserves_real_tls_mode_instead_of_downgrading_to_implicit() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("tls_mode_salvage_test.db");
        let backup_dir = dir.path().join("corrupted_backups");
        let secrets = crate::vault::SecretManager::mock();

        // Step 1: Seed an account configured for STARTTLS on IMAP and plaintext on SMTP --
        // deliberately neither is `ImplicitTls`, so a salvage that hardcodes the TLS mode
        // back to `ImplicitTls` is caught by asserting the real modes survive.
        {
            let engine = DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .unwrap();
            let acct = AccountConfig {
                id: "acct-starttls-1".to_string(),
                name: "STARTTLS Account".to_string(),
                email_address: "starttls@nuncio.mx".to_string(),
                keyring_secret_key: "nuncio/acct-starttls-1".to_string(),
                sync_interval_secs: 60,
                filters_enabled: false,
                transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                    imap_host: "imap.nuncio.mx".to_string(),
                    imap_port: 143,
                    imap_tls_mode: nuncio_core::TlsMode::StartTls,
                    smtp_host: "smtp.nuncio.mx".to_string(),
                    smtp_port: 587,
                    smtp_tls_mode: nuncio_core::TlsMode::Plain,
                }),
            };
            engine.save_account(&acct).await.unwrap();
        }

        // Step 2: Perform salvage recovery.
        let summary = SqliteRecoveryEngine::salvage(&db_path, &db_path, &backup_dir, &secrets)
            .await
            .expect("salvage succeeds");
        assert_eq!(summary.salvaged_accounts_count, 1);

        // Step 3: The salvaged account must keep its real configured TLS modes, not be
        // silently reset to `ImplicitTls` -- that would misrepresent the account's actual,
        // intentionally-configured security posture (STARTTLS on IMAP, plaintext on SMTP).
        let fresh = DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .unwrap();
        let accounts = fresh.list_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        let salvaged_t = accounts[0].imap_smtp().expect("imap-smtp transport");
        assert_eq!(salvaged_t.imap_tls_mode, nuncio_core::TlsMode::StartTls);
        assert_eq!(salvaged_t.smtp_tls_mode, nuncio_core::TlsMode::Plain);
    }

    // NOTE: there is deliberately no test combining *header* corruption (bytes 0-16 -- the
    // fixed "SQLite format 3\0" magic string) with a nonzero salvaged-rule-count assertion.
    // A corrupted header makes the file entirely unopenable by SQLite (`SQLITE_NOTADB`), by
    // design regardless of how intact the rest of the file's pages are, so salvage's backup
    // connection can never read ANY row out of a header-corrupted file -- that is real SQLite
    // behavior, not a salvage bug. `test_database_header_corruption_stage_1_detection` above
    // proves the header-corruption case still triggers detection + backup isolation (no
    // regression in destructive-path gating); `test_stage_2_backup_creation_and_stage_3_table_salvage`
    // above proves the restore INSERT stays schema-correct by exercising salvage's actual read-then-restore SQL
    // against a normally-openable source, which is the only way to test the restore SQL, since
    // header corruption forecloses any row-level read.
}
