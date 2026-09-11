mod journal;
use super::{Engine, EngineConfig, EngineError};
use crate::{
    profile::RestoreIdentity,
    store::{stage_restore_with, RestoreReport, StoreError},
};
pub(super) use journal::recover as recover_restores;
use serde::Serialize;
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Debug, Serialize)]
pub struct ProfileRestoreReport {
    pub profile_id: String,
    pub directory: PathBuf,
    pub restore: RestoreReport,
}
impl Engine {
    pub async fn create_backup(
        &self,
        passphrase: Zeroizing<String>,
    ) -> Result<crate::store::BackupArtifact, StoreError> {
        self.maintenance
            .create(passphrase, self.accounts.http.clock.now_ms())
            .await
    }

    pub async fn begin_backup_upload(
        &self,
        byte_length: u64,
        sha256: String,
    ) -> Result<super::BackupUpload, StoreError> {
        self.maintenance.upload(byte_length, sha256).await
    }

    pub async fn inspect_backup(
        &self,
        input: super::BackupInput,
        passphrase: Zeroizing<String>,
    ) -> Result<crate::store::BackupInspection, StoreError> {
        tokio::task::spawn_blocking(move || {
            let result = crate::store::inspect_backup(&input.path(), passphrase);
            drop(input);
            result
        })
        .await
        .map_err(|_| StoreError::Unavailable)?
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub async fn restore_backup(
        &self,
        input: super::BackupInput,
        passphrase: Zeroizing<String>,
        new_profile: String,
    ) -> Result<ProfileRestoreReport, EngineError> {
        let config = self.maintenance.restore_config(&new_profile)?;
        let now = self.accounts.http.clock.now_ms();
        let profile_lock = self._profile_lock.clone();
        let journal = journal::Journal {
            store: self.store.clone(),
            owner: journal::canonical_owner(&self.directory)?,
            runtime: tokio::runtime::Handle::current(),
        };
        #[cfg(feature = "test-harness")]
        let test_config = self.test_config.clone();
        tokio::task::spawn_blocking(move || {
            // Cancellation can outlive Engine shutdown. Another engine must
            // not recover this profile while its restore is still mutating it.
            let _profile_lock = profile_lock;
            let result = restore(
                config,
                input.path(),
                passphrase,
                now,
                Some(&journal),
                #[cfg(feature = "test-harness")]
                test_config.as_ref(),
            );
            let cleanup = input.cleanup();
            if let Ok(report) = &result {
                cleanup.map_err(|_| StoreError::RestoreActivationUncertain)?;
                // Sync the upload deletion before retiring its durable record.
                std::fs::File::open(&journal.owner)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| StoreError::RestoreActivationUncertain)?;
                match journal
                    .runtime
                    .block_on(journal.store.finish_restore_job(report.profile_id.clone()))
                {
                    Ok(()) | Err(StoreError::Unavailable) => {}
                    Err(_) => return Err(StoreError::RestoreActivationUncertain.into()),
                }
                // Unavailable means the source Store already shut down. The
                // retained row is reconciled at startup, keeping activated keys.
            }
            result
        })
        .await
        .map_err(|_| EngineError::Unavailable)?
    }

    /// Builds a disconnected new profile; it does not start the restored engine.
    /// Blocking restore work owns its resources until completion even if the
    /// awaiting caller disappears. API admission must cover that full lifetime.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub async fn restore_profile(
        config: EngineConfig,
        source: PathBuf,
        passphrase: Zeroizing<String>,
    ) -> Result<ProfileRestoreReport, EngineError> {
        let now = chrono::Utc::now().timestamp_millis();
        tokio::task::spawn_blocking(move || {
            restore(
                config,
                source,
                passphrase,
                now,
                None,
                #[cfg(feature = "test-harness")]
                None,
            )
        })
        .await
        .map_err(|_| EngineError::Unavailable)?
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn restore(
    config: EngineConfig,
    source: PathBuf,
    passphrase: Zeroizing<String>,
    now: i64,
    journal: Option<&journal::Journal>,
    #[cfg(feature = "test-harness")] test_config: Option<&crate::test_controls::TestConfig>,
) -> Result<ProfileRestoreReport, EngineError> {
    match std::fs::symlink_metadata(&config.directory) {
        Ok(_) => return Err(StoreError::Io(std::io::ErrorKind::AlreadyExists).into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(StoreError::from(e).into()),
    }
    let parent = config.directory.parent().ok_or(StoreError::InvalidPath)?;
    let identity = RestoreIdentity::new(config.secrets.as_ref())?;
    let stage = stage_restore_with(
        &source,
        passphrase,
        identity.database_key.clone(),
        parent,
        now,
        |stage, parent, stage_id| {
            if let Some(journal) = journal {
                journal.record(
                    identity.id,
                    &source,
                    &config.directory,
                    stage,
                    parent,
                    stage_id,
                )?;
            }
            #[cfg(feature = "test-harness")]
            if let Some(config) = test_config {
                config
                    .blocking_checkpoint("restore_stage_owned")
                    .map_err(|_| StoreError::Unavailable)?;
            }
            Ok(())
        },
    )?;
    // Persist before keys or rename; a detached worker may finish after its
    // source store shuts down, so it needs no later database write to activate.
    if let Some(journal) = journal {
        journal.activation_intent(identity.id)?;
    }
    #[cfg(feature = "test-harness")]
    if let Some(config) = test_config {
        config
            .blocking_checkpoint("restore_after_stage")
            .map_err(|_| EngineError::Unavailable)?;
    }
    if let Err(error) = identity.initialize(
        stage.directory(),
        config.secrets.as_ref(),
        #[cfg(feature = "test-harness")]
        test_config,
    ) {
        identity.rollback(config.secrets.as_ref())?;
        return Err(error);
    }
    let report = match stage.activate(&config.directory) {
        Ok(report) => report,
        // Activation succeeded: these keys now belong to a real restored
        // profile and must remain available, even when its fsync failed.
        Err(StoreError::RestoreActivationUncertain) => {
            return Err(StoreError::RestoreActivationUncertain.into())
        }
        Err(error) => {
            identity.rollback(config.secrets.as_ref())?;
            return Err(error.into());
        }
    };
    #[cfg(feature = "test-harness")]
    if let Some(config) = test_config {
        config
            .blocking_checkpoint("restore_after_activation")
            .map_err(|_| EngineError::Unavailable)?;
    }
    Ok(ProfileRestoreReport {
        profile_id: identity.id.to_string(),
        directory: config.directory,
        restore: report,
    })
}

#[cfg(test)]
mod journal_tests;
