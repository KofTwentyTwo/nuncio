use super::Manifest;
use crate::{
    domain::identity::ProfileId, engine::EngineError, secrets::SecretStore, store::StoreError,
};
use rand::RngCore;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};
use zeroize::Zeroizing;

pub(crate) struct RestoreIdentity {
    pub id: ProfileId,
    pub database_key: Zeroizing<Vec<u8>>,
    api_key: Zeroizing<Vec<u8>>,
}
impl RestoreIdentity {
    pub fn new(secrets: &dyn SecretStore) -> Result<Self, EngineError> {
        let id = ProfileId::generate();
        for name in names(id) {
            if secrets.get(&name)?.is_some() {
                return Err(EngineError::InvalidProfile);
            }
        }
        let mut database_key = Zeroizing::new(vec![0; 32]);
        let mut api_key = Zeroizing::new(vec![0; 32]);
        rand::rngs::OsRng
            .try_fill_bytes(&mut database_key)
            .map_err(|_| EngineError::Unavailable)?;
        rand::rngs::OsRng
            .try_fill_bytes(&mut api_key)
            .map_err(|_| EngineError::Unavailable)?;
        Ok(Self {
            id,
            database_key,
            api_key,
        })
    }

    /// Only for a private restore stage; ordinary profile startup refuses an
    /// existing store without its manifest and must keep doing so.
    pub fn initialize(
        &self,
        directory: &Path,
        secrets: &dyn SecretStore,
        #[cfg(feature = "test-harness")] test_config: Option<&crate::test_controls::TestConfig>,
    ) -> Result<(), EngineError> {
        let bytes = serde_json::to_vec(&Manifest {
            version: 1,
            id: self.id,
        })
        .map_err(|_| EngineError::InvalidProfile)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(directory.join("profile.json"))
            .map_err(StoreError::from)?;
        file.write_all(&bytes).map_err(StoreError::from)?;
        file.sync_all().map_err(StoreError::from)?;
        let [database_name, api_name] = names(self.id);
        secrets.put(&database_name, &self.database_key)?;
        #[cfg(feature = "test-harness")]
        if let Some(config) = test_config {
            config
                .blocking_checkpoint("restore_after_database_key")
                .map_err(|_| EngineError::Unavailable)?;
        }
        secrets.put(&api_name, &self.api_key)?;
        #[cfg(feature = "test-harness")]
        if let Some(config) = test_config {
            config
                .blocking_checkpoint("restore_after_api_key")
                .map_err(|_| EngineError::Unavailable)?;
        }
        File::open(directory)
            .and_then(|f| f.sync_all())
            .map_err(StoreError::from)?;
        Ok(())
    }

    pub fn rollback(&self, secrets: &dyn SecretStore) -> Result<(), EngineError> {
        // A put may persist before returning an error. Attempt both deletions,
        // including the failed put, without touching any old profile reference.
        rollback(self.id, secrets)
    }
}
pub(crate) fn rollback(id: ProfileId, secrets: &dyn SecretStore) -> Result<(), EngineError> {
    let mut failed = false;
    for name in names(id) {
        failed |= secrets.delete(&name).is_err();
    }
    if failed {
        return Err(EngineError::RestoreCleanupIncomplete {
            profile_id: id.to_string(),
        });
    }
    Ok(())
}
pub(crate) fn names(id: ProfileId) -> [String; 2] {
    [
        format!("{id}/profile/database"),
        format!("{id}/profile/api"),
    ]
}
