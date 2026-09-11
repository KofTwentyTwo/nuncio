#![allow(clippy::unwrap_used, clippy::expect_used)]
use nuncio_engine::{
    engine::{Engine, EngineConfig},
    secrets::{SecretError, SecretStore},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

#[tokio::test]
async fn single_component_relative_profile_activates_an_independent_restored_profile() {
    use sha2::{Digest, Sha256};
    let source = tempfile::tempdir_in(".").unwrap();
    let target = tempfile::Builder::new()
        .prefix("relative-target-")
        .tempdir_in(".")
        .unwrap();
    std::fs::remove_dir(target.path()).unwrap();
    let relative = std::path::PathBuf::from(source.path().file_name().unwrap());
    assert!(relative.parent().unwrap().as_os_str().is_empty());
    let secrets = Arc::new(MemorySecrets::default());
    let engine = Engine::open(EngineConfig {
        directory: relative.clone(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let original_id = engine.status().await.unwrap().profile_id;
    let original_auth = engine.authorization();
    let original_keys = secrets.0.lock().unwrap().clone();
    let phrase: Zeroizing<String> =
        Zeroizing::new("synthetic successful relative restore phrase".into());
    let backup = engine.create_backup(phrase.clone()).await.unwrap();
    let bytes = std::fs::read(backup.path()).unwrap();
    drop(backup);
    let mut upload = engine
        .begin_backup_upload(bytes.len() as u64, format!("{:x}", Sha256::digest(&bytes)))
        .await
        .unwrap();
    for (i, part) in bytes.chunks(262144).enumerate() {
        upload = upload
            .append((i * 262144) as u64, part.to_vec())
            .await
            .unwrap();
    }
    let report = engine
        .restore_backup(
            upload.finish().await.unwrap(),
            phrase,
            target.path().file_name().unwrap().to_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(
        report.directory,
        std::fs::canonicalize(target.path()).unwrap()
    );
    assert_ne!(report.profile_id, original_id);
    assert_eq!(engine.status().await.unwrap().profile_id, original_id);
    engine.shutdown().await.unwrap();
    let restored = Engine::open(EngineConfig {
        directory: target.path().into(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    assert_eq!(
        restored.status().await.unwrap().profile_id,
        report.profile_id
    );
    assert!(restored.authorization() != original_auth);
    restored.shutdown().await.unwrap();
    for (name, key) in original_keys {
        assert!(secrets.0.lock().unwrap()[&name] == key);
    }
    assert_eq!(secrets.0.lock().unwrap().len(), 4);
}

#[derive(Default)]
struct MemorySecrets(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);

impl SecretStore for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        self.0
            .lock()
            .unwrap()
            .insert(name.into(), Zeroizing::new(value.to_vec()));
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

#[tokio::test]
async fn profile_and_distinct_database_api_keys_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let engine = Engine::open(EngineConfig {
        directory: directory.path().into(),
        secrets: secrets.clone(),
    })
    .await;
    assert!(
        engine.is_ok(),
        "engine should initialize an encrypted profile"
    );
    let engine = engine.unwrap();
    let id = engine.status().await.unwrap().profile_id;
    let auth = engine.authorization();
    assert!(auth.starts_with("Bearer "));
    assert_eq!(auth.len(), 71);
    {
        let entries = secrets.0.lock().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            entries[&format!("{id}/profile/database")] != entries[&format!("{id}/profile/api")],
            "database and API keys must differ"
        );
    }
    engine.shutdown().await.unwrap();
    let restarted = Engine::open(EngineConfig {
        directory: directory.path().into(),
        secrets,
    })
    .await
    .unwrap();
    assert_eq!(restarted.status().await.unwrap().profile_id, id);
    assert!(
        restarted.authorization() == auth,
        "restart must retain API authorization"
    );
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_database_key_never_generates_a_replacement_for_existing_data() {
    let directory = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let engine = Engine::open(EngineConfig {
        directory: directory.path().into(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let id = engine.status().await.unwrap().profile_id;
    engine.shutdown().await.unwrap();
    let path = directory.path().join("store.db");
    let original = std::fs::read(&path).unwrap();
    secrets.delete(&format!("{id}/profile/database")).unwrap();
    assert!(Engine::open(EngineConfig {
        directory: directory.path().into(),
        secrets: secrets.clone()
    })
    .await
    .is_err());
    assert!(secrets
        .get(&format!("{id}/profile/database"))
        .unwrap()
        .is_none());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[tokio::test]
async fn profiles_using_one_keystore_have_distinct_authentication() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let a = Engine::open(EngineConfig {
        directory: one.path().into(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let b = Engine::open(EngineConfig {
        directory: two.path().into(),
        secrets,
    })
    .await
    .unwrap();
    assert!(
        a.authorization() != b.authorization(),
        "profiles must have different API credentials"
    );
    assert_ne!(
        a.status().await.unwrap().profile_id,
        b.status().await.unwrap().profile_id
    );
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

#[tokio::test]
async fn relative_profile_directories_report_absolute_protected_paths_including_future_journals() {
    let temp = tempfile::Builder::new()
        .prefix("relative-profile-")
        .tempdir_in(".")
        .unwrap();
    let directory = std::path::PathBuf::from(temp.path().file_name().unwrap());
    assert!(!directory.is_absolute());
    let engine = Engine::open(EngineConfig {
        directory,
        secrets: Arc::new(MemorySecrets::default()),
    })
    .await
    .unwrap();
    let paths = engine.status().await.unwrap().protected_paths;
    assert!(paths.iter().all(|p| p.is_absolute()));
    let canonical = std::fs::canonicalize(temp.path()).unwrap();
    for name in [
        "store.db",
        "store.db-wal",
        "store.db-shm",
        "store.db-journal",
        "store.lock",
        "profile.json",
        "profile.lock",
    ] {
        assert!(
            paths.contains(&canonical.join(name)),
            "unprotected profile file {name}"
        );
    }
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn single_component_relative_profile_recovers_failed_restore_without_changing_original_keys()
{
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir_in(".").unwrap();
    let relative = std::path::PathBuf::from(temp.path().file_name().unwrap());
    assert!(relative.parent().unwrap().as_os_str().is_empty());
    let secrets = Arc::new(MemorySecrets::default());
    let config = || EngineConfig {
        directory: relative.clone(),
        secrets: secrets.clone(),
    };
    let engine = Engine::open(config()).await.unwrap();
    let id = engine.status().await.unwrap().profile_id;
    let manifest = std::fs::read(relative.join("profile.json")).unwrap();
    let keys = secrets.0.lock().unwrap().clone();
    let backup = engine
        .create_backup(Zeroizing::new(
            "synthetic original relative recovery phrase".into(),
        ))
        .await
        .unwrap();
    let bytes = std::fs::read(backup.path()).unwrap();
    drop(backup);
    let mut upload = engine
        .begin_backup_upload(bytes.len() as u64, format!("{:x}", Sha256::digest(&bytes)))
        .await
        .unwrap();
    for (i, part) in bytes.chunks(262144).enumerate() {
        upload = upload
            .append((i * 262144) as u64, part.to_vec())
            .await
            .unwrap();
    }
    let target = format!("relative-test-{}", uuid::Uuid::new_v4());
    let result = engine
        .restore_backup(
            upload.finish().await.unwrap(),
            Zeroizing::new("synthetic incorrect relative recovery phrase".into()),
            target.clone(),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(nuncio_engine::engine::EngineError::Storage(
                nuncio_engine::store::StoreError::KeyOrCorrupt
            ))
        ),
        "restore must reach encrypted backup inspection: {result:?}"
    );
    assert!(!std::path::Path::new(&target).exists());
    engine.shutdown().await.unwrap();
    let pending = || {
        let c = rusqlite::Connection::open(relative.join("store.db")).unwrap();
        let key = Zeroizing::new(format!(
            "x'{}'",
            hex::encode(&keys[&format!("{id}/profile/database")])
        ));
        c.pragma_update(None, "key", key.as_str()).unwrap();
        c.query_row("SELECT count(*) FROM restore_cleanup_jobs", [], |r| {
            r.get::<_, u32>(0)
        })
        .unwrap()
    };
    assert_eq!(pending(), 1, "failed restore must have been journaled");
    let reopened = Engine::open(config()).await.unwrap();
    assert_eq!(reopened.status().await.unwrap().profile_id, id);
    reopened.shutdown().await.unwrap();
    assert_eq!(pending(), 0, "startup must retire the owned cleanup job");
    assert!(*secrets.0.lock().unwrap() == keys);
    assert_eq!(
        std::fs::read(relative.join("profile.json")).unwrap(),
        manifest
    );
}
