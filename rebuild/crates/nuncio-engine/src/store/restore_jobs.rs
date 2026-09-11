use super::{Store, StoreError};
use crate::domain::identity::ProfileId;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, Metadata},
    os::unix::fs::MetadataExt,
    path::Path,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectoryIdentity {
    device: u64,
    inode: u64,
}
impl DirectoryIdentity {
    fn from_metadata(metadata: Metadata) -> Result<Self, StoreError> {
        if !metadata.is_dir() {
            return Err(StoreError::InvalidPath);
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    pub fn at(path: &Path) -> Result<Self, StoreError> {
        Self::from_metadata(std::fs::symlink_metadata(path)?)
    }
    pub fn file(file: &File) -> Result<Self, StoreError> {
        Self::from_metadata(file.metadata()?)
    }
    pub fn matches(self, path: &Path) -> Result<bool, StoreError> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => Ok(metadata.is_dir() && Self::from_metadata(metadata)? == self),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnedDirectory {
    pub name: String,
    pub identity: DirectoryIdentity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreJob {
    pub profile_id: ProfileId,
    pub owner: DirectoryIdentity,
    pub parent: DirectoryIdentity,
    pub stage: OwnedDirectory,
    pub upload: OwnedDirectory,
    pub target: String,
}
impl RestoreJob {
    fn validate(&self) -> Result<(), StoreError> {
        let valid = |name: &str| {
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        };
        if !valid(&self.target)
            || self.target.len() > 64
            || !self.stage.name.strip_prefix(".restore-").is_some_and(valid)
            || !self
                .upload
                .name
                .strip_prefix(".maintenance-")
                .is_some_and(valid)
            || self.owner == self.stage.identity
            || self.owner == self.upload.identity
            || self.parent == self.stage.identity
            || self.stage.identity == self.upload.identity
        {
            return Err(StoreError::InvalidInput);
        }
        Ok(())
    }
}
impl Store {
    pub(crate) async fn record_restore_job(&self, job: RestoreJob) -> Result<(), StoreError> {
        job.validate()?;
        self.execute(move |c| {
            let tx = c.transaction()?;
            if tx.query_row("SELECT count(*) FROM restore_cleanup_jobs", [], |r| {
                r.get::<_, u32>(0)
            })? >= 64
            {
                return Err(StoreError::Busy);
            }
            tx.execute(
                "INSERT INTO restore_cleanup_jobs(profile_id,record_json) VALUES(?1,?2)",
                params![
                    job.profile_id.to_string(),
                    serde_json::to_string(&job).map_err(|_| StoreError::InvalidInput)?
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub(crate) async fn activate_restore_job(&self, id: String) -> Result<(), StoreError> {
        self.execute(move |c| {
            if c.execute(
                "UPDATE restore_cleanup_jobs SET activating=1 WHERE profile_id=?1",
                [id],
            )? != 1
            {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
        .await
    }
    pub(crate) async fn restore_jobs(&self) -> Result<Vec<(RestoreJob, bool)>, StoreError> {
        self.execute(|c|{
            let rows=c.prepare("SELECT profile_id,record_json,activating FROM restore_cleanup_jobs ORDER BY profile_id LIMIT 65")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
            if rows.len()>64 {return Err(StoreError::ResultTooLarge);}
            rows.into_iter().map(|(id,json,activating)|{
                if json.len()>4096 {return Err(StoreError::KeyOrCorrupt);}
                let job:RestoreJob=serde_json::from_str(&json).map_err(|_|StoreError::KeyOrCorrupt)?;
                job.validate().map_err(|_|StoreError::KeyOrCorrupt)?;
                if job.profile_id.to_string()!=id {return Err(StoreError::KeyOrCorrupt);}
                Ok((job,activating))
            }).collect()
        }).await
    }
    pub(crate) async fn finish_restore_job(&self, id: String) -> Result<(), StoreError> {
        self.execute(move |c| {
            c.execute("DELETE FROM restore_cleanup_jobs WHERE profile_id=?1", [id])?;
            Ok(())
        })
        .await
    }
}
