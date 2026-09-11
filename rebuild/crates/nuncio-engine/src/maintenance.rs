use crate::{
    engine::EngineConfig,
    secrets::SecretStore,
    store::{
        BackupArtifact, CleanupDirectory, DirectoryIdentity, OwnedDirectory, Store, StoreError,
    },
};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use zeroize::Zeroizing;

use crate::store::MAX_BACKUP_BYTES;
const CHUNK_BYTES: usize = 262144;

pub(crate) struct Maintenance {
    directory: PathBuf,
    secrets: Arc<dyn SecretStore>,
    store: Store,
    admission: Arc<Semaphore>,
    profile_lock: Arc<File>,
}

pub(crate) struct MaintenanceLease {
    _permit: OwnedSemaphorePermit,
    _profile_lock: Arc<File>,
}
impl Maintenance {
    pub fn new(
        directory: PathBuf,
        secrets: Arc<dyn SecretStore>,
        store: Store,
        profile_lock: Arc<File>,
    ) -> Self {
        Self {
            directory,
            secrets,
            store,
            admission: Arc::new(Semaphore::new(1)),
            profile_lock,
        }
    }
    fn lease(&self) -> Result<MaintenanceLease, StoreError> {
        Ok(MaintenanceLease {
            _permit: self
                .admission
                .clone()
                .try_acquire_owned()
                .map_err(|_| StoreError::Busy)?,
            _profile_lock: self.profile_lock.clone(),
        })
    }
    pub async fn create(
        &self,
        phrase: Zeroizing<String>,
        now: i64,
    ) -> Result<BackupArtifact, StoreError> {
        let lease = self.lease()?;
        let result = self
            .store
            .create_backup_with_guard(phrase, now, Some(lease))
            .await?;
        if result.inspection().byte_length > MAX_BACKUP_BYTES {
            return Err(StoreError::ResultTooLarge);
        }
        Ok(result)
    }
    pub async fn upload(&self, length: u64, hash: String) -> Result<BackupUpload, StoreError> {
        if length == 0
            || length > MAX_BACKUP_BYTES
            || hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(StoreError::InvalidInput);
        }
        let lease = self.lease()?;
        let parent = self.directory.clone();
        tokio::task::spawn_blocking(move || {
            crate::store::backup_preflight(&parent, length, 1)?;
            let directory = tempfile::Builder::new()
                .prefix(".maintenance-")
                .tempdir_in(&parent)?;
            let parent_identity = DirectoryIdentity::at(&parent)?;
            let owned = OwnedDirectory {
                name: directory
                    .path()
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or(StoreError::InvalidPath)?
                    .to_owned(),
                identity: DirectoryIdentity::at(directory.path())?,
            };
            File::open(&parent)?.sync_all()?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(directory.path().join("upload.nuncio"))?;
            Ok(BackupUpload {
                file,
                directory,
                parent_identity,
                owned,
                lease,
                length,
                expected_hash: hash,
                received: 0,
                hash: Sha256::new(),
            })
        })
        .await
        .map_err(|_| StoreError::Unavailable)?
    }
    pub fn restore_config(&self, name: &str) -> Result<EngineConfig, StoreError> {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(StoreError::InvalidInput);
        }
        let directory = std::fs::canonicalize(&self.directory)?;
        let parent = directory.parent().ok_or(StoreError::InvalidPath)?;
        Ok(EngineConfig {
            directory: parent.join(name),
            secrets: self.secrets.clone(),
        })
    }
}

/// A single ordered upload. Each blocking write owns the file, private directory
/// and admission permit, including after cancellation of its awaiting caller.
pub struct BackupUpload {
    file: File,
    directory: tempfile::TempDir,
    parent_identity: DirectoryIdentity,
    owned: OwnedDirectory,
    lease: MaintenanceLease,
    length: u64,
    expected_hash: String,
    received: u64,
    hash: Sha256,
}
impl BackupUpload {
    pub async fn append(mut self, offset: u64, bytes: Vec<u8>) -> Result<Self, StoreError> {
        if offset != self.received
            || bytes.is_empty()
            || bytes.len() > CHUNK_BYTES
            || self
                .received
                .checked_add(bytes.len() as u64)
                .is_none_or(|n| n > self.length)
        {
            return Err(StoreError::InvalidInput);
        }
        tokio::task::spawn_blocking(move || {
            self.file.write_all(&bytes)?;
            self.hash.update(&bytes);
            self.received += bytes.len() as u64;
            Ok(self)
        })
        .await
        .map_err(|_| StoreError::Unavailable)?
    }
    pub async fn finish(self) -> Result<BackupInput, StoreError> {
        tokio::task::spawn_blocking(move || {
            if self.received != self.length
                || format!("{:x}", self.hash.clone().finalize()) != self.expected_hash
            {
                return Err(StoreError::InvalidInput);
            }
            self.file.sync_all()?;
            drop(self.file);
            File::open(self.directory.path())?.sync_all()?;
            Ok(BackupInput {
                directory: self.directory,
                parent_identity: self.parent_identity,
                owned: self.owned,
                _lease: self.lease,
            })
        })
        .await
        .map_err(|_| StoreError::Unavailable)?
    }
}

/// A checksum-verified encrypted input. It retains admission through inspection
/// or restore; neither its path nor arbitrary caller files are accepted by RPCs.
pub struct BackupInput {
    directory: tempfile::TempDir,
    parent_identity: DirectoryIdentity,
    owned: OwnedDirectory,
    _lease: MaintenanceLease,
}
impl BackupInput {
    pub(crate) fn cleanup(mut self) -> Result<(), StoreError> {
        self.directory.disable_cleanup(true);
        let parent = self
            .directory
            .path()
            .parent()
            .ok_or(StoreError::InvalidPath)?;
        if let Some(directory) = CleanupDirectory::prepare(
            parent,
            self.parent_identity,
            &self.owned,
            &["upload.nuncio"],
        )? {
            directory.remove()?;
        }
        Ok(())
    }
    pub(crate) fn path(&self) -> PathBuf {
        self.directory.path().join("upload.nuncio")
    }
}
