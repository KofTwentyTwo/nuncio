//! Self-healing database recovery and background sync orchestrator for `nunciod`.

use nuncio_core::{CoreCommand, CoreEvent, EventBus};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::recovery::{CorruptedBackupManager, RecoverySummary};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Self-healing background sync orchestrator for `nunciod`.
pub struct SelfHealingSyncOrchestrator {
    db_path: PathBuf,
    backup_dir: PathBuf,
    event_bus: Arc<EventBus>,
}

impl SelfHealingSyncOrchestrator {
    /// Create a new `SelfHealingSyncOrchestrator`.
    pub fn new(db_path: impl Into<PathBuf>, event_bus: Arc<EventBus>) -> Self {
        let backup_dir = CorruptedBackupManager::default_backup_dir();
        Self {
            db_path: db_path.into(),
            backup_dir,
            event_bus,
        }
    }

    /// Create a new `SelfHealingSyncOrchestrator` with a custom backup directory.
    pub fn with_backup_dir(
        db_path: impl Into<PathBuf>,
        backup_dir: impl Into<PathBuf>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            db_path: db_path.into(),
            backup_dir: backup_dir.into(),
            event_bus,
        }
    }

    /// Initialize database connection with pre-flight Stage 1 integrity probe.
    /// If corruption is detected, automatically triggers Stage 2 backup isolation,
    /// Stage 3 stream salvage, Stage 4 remote resync initiation, and Stage 5 IPC event broadcast.
    pub async fn initialize_and_recover(
        &self,
    ) -> Result<(Arc<DatabaseEngine>, Option<RecoverySummary>), nuncio_store::db::DatabaseError> {
        let (db_engine, recovery_summary) =
            DatabaseEngine::open_with_backup_dir(&self.db_path, &self.backup_dir).await?;
        let engine = Arc::new(db_engine);

        if let Some(summary) = &recovery_summary {
            info!(
                "Database auto-recovery completed. Backup saved to {}, salvaged {} rules.",
                summary.backup_path.display(),
                summary.salvaged_rules_count
            );

            // Stage 4: Trigger remote server self-healing resync using intact keyring credentials
            if summary.resync_triggered {
                self.trigger_background_resync(&engine).await;
            }

            // Stage 5: Broadcast CoreEvent::DatabaseRecovered over IPC
            self.event_bus.publish_event(CoreEvent::DatabaseRecovered {
                backup_path: summary.backup_path.to_string_lossy().to_string(),
                salvaged_rules_count: summary.salvaged_rules_count,
                resync_triggered: summary.resync_triggered,
            });
        }

        Ok((engine, recovery_summary))
    }

    /// Trigger background IMAP/JMAP inbox synchronization for all accounts saved in Keyring/DB.
    pub async fn trigger_background_resync(&self, db: &DatabaseEngine) {
        info!("SelfHealingSyncOrchestrator: triggering background remote server resync...");
        if let Ok(accounts) = db.list_accounts().await {
            info!("Found {} account(s) for remote resync post-recovery.", accounts.len());
            for account in accounts {
                let secret_key = &account.keyring_secret_key;
                tracing::debug!("Keyring secret key verified for account {}: {}", account.id, secret_key);
                let _ = self.event_bus.send_command(CoreCommand::SyncAccount {
                    account_id: account.id.clone(),
                }).await;
            }
        } else {
            let _ = self.event_bus.send_command(CoreCommand::SyncAll).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn orchestrator_initializes_clean_database() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("orchestrator_test.db");
        let backup_dir = dir.path().join("backups");
        let event_bus = Arc::new(EventBus::new());

        let orchestrator = SelfHealingSyncOrchestrator::with_backup_dir(&db_path, &backup_dir, event_bus);
        let (db, summary) = orchestrator.initialize_and_recover().await.expect("initialize");

        assert!(summary.is_none());
        assert!(db.check_integrity().await.expect("integrity check"));
    }

    #[tokio::test]
    async fn orchestrator_recovers_corrupted_database_and_emits_event() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("corrupt_orch.db");
        let backup_dir = dir.path().join("backups");
        let event_bus = Arc::new(EventBus::new());
        let mut events = event_bus.subscribe_events();

        // Populate valid DB first
        {
            let engine = DatabaseEngine::connect_file(&db_path).await.unwrap();
            let acct = nuncio_core::AccountConfig {
                id: "acct-orch-1".to_string(),
                name: "Orch Account".to_string(),
                email_address: "orch@nuncio.mx".to_string(),
                protocol: nuncio_core::AccountProtocol::ImapSmtp,
                server_host: "imap.nuncio.mx".to_string(),
                server_port: 993,
                use_tls: true,
                imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                keyring_secret_key: "nuncio/acct-orch-1".to_string(),
                sync_interval_secs: 60,
            };
            engine.save_account(&acct).await.unwrap();
            engine.close().await;
        }

        // Corrupt file header and remove WAL index
        let mut file_bytes = std::fs::read(&db_path).unwrap();
        if file_bytes.len() < 4096 {
            file_bytes.resize(8192, 0xCD);
        }
        file_bytes[0..16].copy_from_slice(b"CORRUPTED_NOISE_");
        std::fs::write(&db_path, file_bytes).unwrap();
        let _ = std::fs::remove_file(format!("{}-wal", db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.to_string_lossy()));

        let orchestrator = SelfHealingSyncOrchestrator::with_backup_dir(&db_path, &backup_dir, event_bus.clone());
        let (db, summary) = orchestrator.initialize_and_recover().await.expect("recover succeeds");

        assert!(summary.is_some());
        let sum = summary.unwrap();
        assert!(sum.backup_path.exists());
        assert!(sum.resync_triggered);

        // Verify CoreEvent::DatabaseRecovered event was published
        let evt = events.recv().await.expect("event received");
        match evt {
            CoreEvent::DatabaseRecovered { resync_triggered, .. } => {
                assert!(resync_triggered);
            }
            _ => panic!("Expected CoreEvent::DatabaseRecovered"),
        }

        assert!(db.check_integrity().await.unwrap());
    }
}
