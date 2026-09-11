use crate::{
    domain::identity::ProfileId,
    engine::EngineError,
    secrets::SecretStore,
    store::{private_directory, StoreError},
};
use fs4::fs_std::FileExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use zeroize::Zeroizing;
mod restore;
pub(crate) use restore::{rollback as rollback_restore_keys, RestoreIdentity};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    id: ProfileId,
}

pub(crate) struct Profile {
    pub id: ProfileId,
    pub database_key: Zeroizing<Vec<u8>>,
    pub authorization: Zeroizing<String>,
    pub lock: File,
}

pub(crate) fn prepare(directory: &Path, secrets: &dyn SecretStore) -> Result<Profile, EngineError> {
    private_directory(directory)?;
    let lock_path = directory.join("profile.lock");
    reject_symlink(&lock_path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(&lock_path).map_err(StoreError::from)?;
    if !lock.try_lock_exclusive().map_err(StoreError::from)? {
        return Err(StoreError::Locked.into());
    }
    let path = directory.join("profile.json");
    reject_symlink(&path)?;
    let existing_database = directory.join("store.db").exists();
    let manifest = if path.exists() {
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(StoreError::from)?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(StoreError::from)?;
        if bytes.len() > 4096 {
            return Err(EngineError::InvalidProfile);
        }
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|_| EngineError::InvalidProfile)?;
        if manifest.version != 1 {
            return Err(EngineError::InvalidProfile);
        }
        manifest
    } else {
        if existing_database {
            return Err(EngineError::InvalidProfile);
        }
        let manifest = Manifest {
            version: 1,
            id: ProfileId::generate(),
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|_| EngineError::InvalidProfile)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(StoreError::from)?;
        file.write_all(&bytes).map_err(StoreError::from)?;
        file.sync_all().map_err(StoreError::from)?;
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(StoreError::from)?;
        manifest
    };
    let database_key = key(
        secrets,
        &format!("{}/profile/database", manifest.id),
        !existing_database,
    )?;
    let api_key = key(secrets, &format!("{}/profile/api", manifest.id), true)?;
    let authorization = Zeroizing::new(format!("Bearer {}", hex::encode(api_key.as_slice())));
    Ok(Profile {
        id: manifest.id,
        database_key,
        authorization,
        lock,
    })
}

fn key(
    secrets: &dyn SecretStore,
    name: &str,
    may_create: bool,
) -> Result<Zeroizing<Vec<u8>>, EngineError> {
    if let Some(value) = secrets.get(name)? {
        if value.len() != 32 {
            return Err(EngineError::MissingKey);
        }
        return Ok(value);
    }
    if !may_create {
        return Err(EngineError::MissingKey);
    }
    let mut value = Zeroizing::new(vec![0; 32]);
    rand::rngs::OsRng
        .try_fill_bytes(&mut value)
        .map_err(|_| EngineError::Unavailable)?;
    secrets.put(name, &value)?;
    Ok(value)
}

fn reject_symlink(path: &Path) -> Result<(), EngineError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            Err(EngineError::InvalidProfile)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::from(error).into()),
    }
}
