use crate::store::CleanupDirectory;
use crate::{
    domain::identity::ProfileId,
    engine::EngineError,
    secrets::SecretStore,
    store::{DirectoryIdentity, OwnedDirectory, RestoreJob, Store, StoreError},
};
use rustix::fs::{Mode, OFlags};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) struct Journal {
    pub store: Store,
    pub owner: PathBuf,
    pub runtime: tokio::runtime::Handle,
}
impl Journal {
    pub fn record(
        &self,
        id: ProfileId,
        source: &Path,
        target: &Path,
        stage: &Path,
        parent: DirectoryIdentity,
        identity: DirectoryIdentity,
    ) -> Result<(), StoreError> {
        let upload = source.parent().ok_or(StoreError::InvalidPath)?;
        let owner = upload.parent().ok_or(StoreError::InvalidPath)?;
        if canonical_owner(owner)? != self.owner {
            return Err(StoreError::InvalidPath);
        }
        let job = RestoreJob {
            profile_id: id,
            owner: DirectoryIdentity::at(&self.owner)?,
            parent,
            stage: OwnedDirectory {
                name: name(stage)?,
                identity,
            },
            upload: OwnedDirectory {
                name: name(upload)?,
                identity: DirectoryIdentity::at(upload)?,
            },
            target: name(target)?,
        };
        self.runtime.block_on(self.store.record_restore_job(job))
    }
    pub fn activation_intent(&self, id: ProfileId) -> Result<(), StoreError> {
        self.runtime
            .block_on(self.store.activate_restore_job(id.to_string()))
    }
}
fn name(path: &Path) -> Result<String, StoreError> {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .ok_or(StoreError::InvalidPath)
}

pub(super) fn canonical_owner(owner: &Path) -> Result<PathBuf, StoreError> {
    let identity = DirectoryIdentity::at(owner)?;
    let canonical = std::fs::canonicalize(owner)?;
    if !identity.matches(&canonical)? {
        return Err(StoreError::InvalidPath);
    }
    Ok(canonical)
}

pub(in crate::engine) async fn recover(
    store: &Store,
    owner: PathBuf,
    source_id: ProfileId,
    secrets: Arc<dyn SecretStore>,
    profile_lock: Arc<File>,
) -> Result<(), EngineError> {
    let owner = canonical_owner(&owner)?;
    for (job, activating) in store.restore_jobs().await? {
        if job.profile_id == source_id {
            return Err(StoreError::KeyOrCorrupt.into());
        }
        let id = job.profile_id.to_string();
        let directory = owner.clone();
        let secrets = secrets.clone();
        let profile_lock = profile_lock.clone();
        tokio::task::spawn_blocking(move || {
            let _profile_lock = profile_lock;
            cleanup(&job, activating, &directory, secrets.as_ref())
        })
        .await
        .map_err(|_| EngineError::Unavailable)??;
        store.finish_restore_job(id).await?;
    }
    Ok(())
}

fn pending(job: &RestoreJob) -> EngineError {
    EngineError::RestoreRecoveryPending {
        profile_id: job.profile_id.to_string(),
    }
}
fn cleanup(
    job: &RestoreJob,
    activating: bool,
    owner: &Path,
    secrets: &dyn SecretStore,
) -> Result<(), EngineError> {
    let parent = owner.parent().ok_or(StoreError::InvalidPath)?;
    if !job.owner.matches(owner)? || !job.parent.matches(parent)? {
        return Err(pending(job));
    }
    let stage = parent.join(&job.stage.name);
    let target = parent.join(&job.target);
    let activated = job.stage.identity.matches(&target)?;
    if activated {
        if !activating || !manifest_matches(&target, job.profile_id)? {
            return Err(pending(job));
        }
        // The renamed directory owns both keys, including after a lost reply.
    } else {
        let stage_owned = job.stage.identity.matches(&stage)?;
        if (!stage_owned && exists(&stage)?)
            || (activating && !stage_owned && keys_exist(job, secrets)?)
        {
            return Err(pending(job));
        }
        let upload = CleanupDirectory::prepare(owner, job.owner, &job.upload, &["upload.nuncio"])
            .map_err(|_| pending(job))?;
        let stage = CleanupDirectory::prepare(
            parent,
            job.parent,
            &job.stage,
            &[
                "source.nuncio",
                "source.nuncio-wal",
                "source.nuncio-shm",
                "source.nuncio-journal",
                "store.db",
                "store.db-wal",
                "store.db-shm",
                "store.db-journal",
                "store.lock",
                "profile.json",
            ],
        )
        .map_err(|_| pending(job))?;
        crate::profile::rollback_restore_keys(job.profile_id, secrets)?;
        if let Some(stage) = stage {
            stage.remove()?;
        }
        if let Some(upload) = upload {
            upload.remove()?;
        }
        return Ok(());
    }
    if let Some(upload) =
        CleanupDirectory::prepare(owner, job.owner, &job.upload, &["upload.nuncio"])
            .map_err(|_| pending(job))?
    {
        upload.remove()?;
    }
    Ok(())
}
fn keys_exist(job: &RestoreJob, secrets: &dyn SecretStore) -> Result<bool, EngineError> {
    for suffix in ["database", "api"] {
        if secrets
            .get(&format!("{}/profile/{suffix}", job.profile_id))?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn exists(path: &Path) -> Result<bool, StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn open_directory(path: &Path) -> Result<File, StoreError> {
    Ok(File::from(
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}
fn manifest_matches(directory: &Path, id: ProfileId) -> Result<bool, StoreError> {
    let dir = open_directory(directory)?;
    let file = File::from(
        rustix::fs::openat(
            &dir,
            "profile.json",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    if !file.metadata()?.is_file() {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Ok(false);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| StoreError::KeyOrCorrupt)?;
    Ok(value["version"] == 1 && value["id"].as_str() == Some(id.to_string().as_str()))
}
