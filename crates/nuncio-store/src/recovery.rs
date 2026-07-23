//! Database corruption detection, backup isolation, and stream recovery salvage engine.

use crate::db::{DatabaseEngine, DatabaseError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

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
    /// Indicates whether remote protocol resynchronization was initiated.
    pub resync_triggered: bool,
}

/// Helper identifying SQLite corruption and connection error codes.
pub fn is_sqlite_corruption_error(err: &sqlx::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    if msg.contains("disk image is malformed")
        || msg.contains("not a database")
        || msg.contains("corrupt")
        || msg.contains("sqlite_notadb")
        || msg.contains("cantopen")
        || msg.contains("disk i/o error")
    {
        return true;
    }
    if let sqlx::Error::Database(db_err) = err {
        if let Some(code) = db_err.code() {
            let code_str = code.as_ref();
            if code_str == "11" || code_str == "26" || code_str == "14" {
                return true;
            }
        }
    }
    false
}

/// Manager isolating damaged database files into forensic backups.
#[derive(Debug, Clone)]
pub struct CorruptedBackupManager;

impl CorruptedBackupManager {
    /// Default root backup directory (`~/.nuncio/corrupted_backups`).
    pub fn default_backup_dir() -> PathBuf {
        if let Ok(home_str) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            PathBuf::from(home_str).join(".nuncio").join("corrupted_backups")
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
    pub fn backup_corrupted_db_to(db_path: &Path, backup_dir: &Path) -> Result<PathBuf, DatabaseError> {
        std::fs::create_dir_all(backup_dir)
            .map_err(|e| DatabaseError::RecoveryFailed(format!("failed to create backup dir: {e}")))?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let filename = format!("nuncio_corrupted_{}.db", timestamp);
        let target_path = backup_dir.join(&filename);

        if db_path.exists() {
            std::fs::copy(db_path, &target_path)
                .map_err(|e| DatabaseError::RecoveryFailed(format!("failed to copy db file: {e}")))?;
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

        info!("Corrupted database isolated to forensic backup: {}", target_path.display());
        Ok(target_path)
    }
}

/// Stream salvage recovery engine extracting valid metadata tables into a fresh SQLite file.
pub struct SqliteRecoveryEngine;

impl SqliteRecoveryEngine {
    /// Salvage readable accounts and filter rules from damaged database or backup file,
    /// re-creating a fresh SQLite file at `target_db_path`.
    pub async fn salvage(
        corrupted_db_path: &Path,
        target_db_path: &Path,
        backup_dir: &Path,
    ) -> Result<RecoverySummary, DatabaseError> {
        // Step 1: Preserve raw corrupted database to forensic backup directory
        let backup_path = CorruptedBackupManager::backup_corrupted_db_to(corrupted_db_path, backup_dir)?;

        // Step 2: Attempt reading valid records from preserved backup file
        let backup_url = format!("sqlite://{}", backup_path.to_string_lossy());
        let (salvaged_accounts, salvaged_rules, salvaged_conditions, salvaged_actions) =
            if let Ok(pool) = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect(&backup_url).await {
                let accounts = Self::salvage_accounts(&pool).await;
                let (rules, conditions, actions) = Self::salvage_filter_tables(&pool).await;
                pool.close().await;
                (accounts, rules, conditions, actions)
            } else {
                warn!("Failed to open backup connection for salvage; proceeding with clean database reset.");
                (Vec::new(), Vec::new(), Vec::new(), Vec::new())
            };

        // Step 3: Remove or truncate damaged database files at target_db_path
        let _ = std::fs::remove_file(target_db_path);
        let _ = std::fs::OpenOptions::new().write(true).truncate(true).open(target_db_path);
        let _ = std::fs::remove_file(format!("{}-wal", target_db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", target_db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-wal", target_db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", target_db_path.to_string_lossy()));

        // Step 4: Create fresh SQLite database file and apply migrations
        let fresh_engine = DatabaseEngine::connect_file(target_db_path).await?;

        // Step 5: Restore salvaged records into new database
        let mut restored_accounts_count = 0;
        for acct in &salvaged_accounts {
            if fresh_engine.save_account(acct).await.is_ok() {
                restored_accounts_count += 1;
            }
        }

        let mut restored_rules_count = 0;
        for (id, name, desc, enabled, match_all, priority, created_at, updated_at) in &salvaged_rules {
            let res = sqlx::query(
                "INSERT INTO filter_rules (id, name, description, enabled, match_all, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(id)
            .bind(name)
            .bind(desc)
            .bind(enabled)
            .bind(match_all)
            .bind(priority)
            .bind(created_at)
            .bind(updated_at)
            .execute(fresh_engine.pool())
            .await;
            if res.is_ok() {
                restored_rules_count += 1;
            }
        }

        let mut restored_conditions_count = 0;
        for (id, rule_id, field, operator, value) in &salvaged_conditions {
            let res = sqlx::query(
                "INSERT INTO filter_conditions (id, rule_id, field, operator, value) VALUES (?, ?, ?, ?, ?)"
            )
            .bind(id)
            .bind(rule_id)
            .bind(field)
            .bind(operator)
            .bind(value)
            .execute(fresh_engine.pool())
            .await;
            if res.is_ok() {
                restored_conditions_count += 1;
            }
        }

        let mut restored_actions_count = 0;
        for (id, rule_id, action_type, value) in &salvaged_actions {
            let res = sqlx::query(
                "INSERT INTO filter_actions (id, rule_id, action_type, value) VALUES (?, ?, ?, ?)"
            )
            .bind(id)
            .bind(rule_id)
            .bind(action_type)
            .bind(value)
            .execute(fresh_engine.pool())
            .await;
            if res.is_ok() {
                restored_actions_count += 1;
            }
        }

        let summary = RecoverySummary {
            backup_path,
            salvaged_accounts_count: restored_accounts_count,
            salvaged_rules_count: restored_rules_count,
            salvaged_conditions_count: restored_conditions_count,
            salvaged_actions_count: restored_actions_count,
            resync_triggered: true,
        };

        info!("SqliteRecoveryEngine salvage finished: {:?}", summary);
        Ok(summary)
    }

    async fn salvage_accounts(pool: &sqlx::SqlitePool) -> Vec<nuncio_core::AccountConfig> {
        let rows: Result<Vec<(String, String, String, String, String, i64, i64, String, i64)>, _> = sqlx::query_as(
            "SELECT id, name, email_address, protocol, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs FROM accounts"
        )
        .fetch_all(pool)
        .await;

        match rows {
            Ok(vec) => vec
                .into_iter()
                .map(|(id, name, email_address, protocol_str, server_host, server_port, use_tls, keyring_secret_key, sync_interval_secs)| {
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
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    #[allow(clippy::type_complexity)]
    async fn salvage_filter_tables(
        pool: &sqlx::SqlitePool,
    ) -> (
        Vec<(String, String, String, i64, i64, i64, i64, i64)>,
        Vec<(String, String, String, String, String)>,
        Vec<(String, String, String, Option<String>)>,
    ) {
        let rules = sqlx::query_as::<_, (String, String, String, i64, i64, i64, i64, i64)>(
            "SELECT id, name, description, enabled, match_all, priority, created_at, updated_at FROM filter_rules"
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let conditions = sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT id, rule_id, field, operator, value FROM filter_conditions"
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let actions = sqlx::query_as::<_, (String, String, String, Option<String>)>(
            "SELECT id, rule_id, action_type, value FROM filter_actions"
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

    #[tokio::test]
    async fn test_database_header_corruption_stage_1_detection() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("corrupt_test.db");

        // Step A: Create and populate valid database
        {
            let engine = DatabaseEngine::connect_file(&db_path).await.unwrap();
            let acct = AccountConfig {
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
        let (recovered_engine, summary) = DatabaseEngine::open_with_backup_dir(&db_path, &backup_dir)
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

        // Step 1: Create DB with account & rule
        {
            let engine = DatabaseEngine::connect_file(&db_path).await.unwrap();
            let acct = AccountConfig {
                id: "acct-salvage-1".to_string(),
                name: "Salvage Account".to_string(),
                email_address: "salvage@nuncio.mx".to_string(),
                protocol: nuncio_core::AccountProtocol::ImapSmtp,
                server_host: "imap.nuncio.mx".to_string(),
                server_port: 993,
                use_tls: true,
                imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                keyring_secret_key: "nuncio/acct-salvage-1".to_string(),
                sync_interval_secs: 60,
            };
            engine.save_account(&acct).await.unwrap();
        }

        // Step 2: Perform salvage recovery
        let summary = SqliteRecoveryEngine::salvage(&db_path, &db_path, &backup_dir)
            .await
            .expect("salvage succeeds");

        assert_eq!(summary.salvaged_accounts_count, 1);
        assert!(summary.backup_path.exists());

        // Step 3: Verify fresh DB contains salvaged account
        let fresh = DatabaseEngine::connect_file(&db_path).await.unwrap();
        let accounts = fresh.list_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acct-salvage-1");
    }
}
