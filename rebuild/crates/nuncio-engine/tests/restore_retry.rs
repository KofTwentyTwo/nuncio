#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    engine::{BackupInput, Engine, EngineConfig, EngineError},
    secrets::{SecretError, SecretStore},
    store::StoreError,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

#[derive(Default)]
struct Secrets(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);
impl SecretStore for Secrets {
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
async fn upload(engine: &Engine, bytes: &[u8]) -> BackupInput {
    let mut input = engine
        .begin_backup_upload(bytes.len() as u64, format!("{:x}", Sha256::digest(bytes)))
        .await
        .unwrap();
    for (i, part) in bytes.chunks(262144).enumerate() {
        input = input
            .append((i * 262144) as u64, part.to_vec())
            .await
            .unwrap();
    }
    input.finish().await.unwrap()
}

#[tokio::test]
async fn failed_restores_do_not_exhaust_cleanup_capacity_or_require_a_restart() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let secrets = Arc::new(Secrets::default());
    let engine = Engine::open(EngineConfig {
        directory: source.clone(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let original_id = engine.status().await.unwrap().profile_id;
    let original_keys = secrets.0.lock().unwrap().clone();
    let manifest = std::fs::read(source.join("profile.json")).unwrap();
    let phrase: Zeroizing<String> = Zeroizing::new("synthetic retry recovery phrase".into());
    let backup = engine.create_backup(phrase.clone()).await.unwrap();
    let bytes = std::fs::read(backup.path()).unwrap();
    let original = temp.path().join("original.nuncio");
    std::fs::write(&original, &bytes).unwrap();
    drop(backup);
    for attempt in 0..65 {
        let input = upload(&engine, &std::fs::read(&original).unwrap()).await;
        let result = engine
            .restore_backup(
                input,
                Zeroizing::new("synthetic incorrect recovery phrase".into()),
                "restored".into(),
            )
            .await;
        assert!(
            matches!(result, Err(EngineError::Storage(StoreError::KeyOrCorrupt))),
            "attempt {attempt} must reach backup validation: {result:?}"
        );
        assert!(!temp.path().join("restored").exists());
        assert!(*secrets.0.lock().unwrap() == original_keys);
    }
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    assert_eq!(
        std::fs::read(source.join("profile.json")).unwrap(),
        manifest
    );
    assert_eq!(engine.status().await.unwrap().profile_id, original_id);
    let report = engine
        .restore_backup(upload(&engine, &bytes).await, phrase, "restored".into())
        .await
        .unwrap();
    assert_ne!(report.profile_id, original_id);
    engine.shutdown().await.unwrap();
    let restored = Engine::open(EngineConfig {
        directory: report.directory,
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    assert_eq!(
        restored.status().await.unwrap().profile_id,
        report.profile_id
    );
    restored.shutdown().await.unwrap();
    for (name, value) in original_keys {
        assert!(secrets.0.lock().unwrap()[&name] == value);
    }
    assert_eq!(secrets.0.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn another_profiles_upload_cannot_trigger_recovery_of_this_profiles_pending_jobs() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(Secrets::default());
    let a = Engine::open(EngineConfig {
        directory: temp.path().join("a"),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let b = Engine::open(EngineConfig {
        directory: temp.path().join("b"),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let id = b.status().await.unwrap().profile_id;
    let original_keys = secrets.0.lock().unwrap().clone();
    let phrase: Zeroizing<String> =
        Zeroizing::new("synthetic cross profile recovery phrase".into());
    let backup = b.create_backup(phrase.clone()).await.unwrap();
    let bytes = std::fs::read(backup.path()).unwrap();
    drop(backup);
    let result = b
        .restore_backup(
            upload(&b, &bytes).await,
            Zeroizing::new("synthetic incorrect cross profile phrase".into()),
            "restored".into(),
        )
        .await;
    assert!(matches!(
        result,
        Err(EngineError::Storage(StoreError::KeyOrCorrupt))
    ));
    let pending = || {
        let c = rusqlite::Connection::open_with_flags(
            temp.path().join("b/store.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let key = Zeroizing::new(format!(
            "x'{}'",
            hex::encode(&original_keys[&format!("{id}/profile/database")])
        ));
        c.pragma_update(None, "key", key.as_str()).unwrap();
        c.query_row("SELECT count(*) FROM restore_cleanup_jobs", [], |r| {
            r.get::<_, u32>(0)
        })
        .unwrap()
    };
    assert_eq!(pending(), 1);
    let result = b
        .restore_backup(upload(&a, &bytes).await, phrase, "restored".into())
        .await;
    assert!(matches!(
        result,
        Err(EngineError::Storage(StoreError::InvalidPath))
    ));
    assert_eq!(
        pending(),
        1,
        "foreign upload must not retire this profile's cleanup jobs"
    );
    assert!(*secrets.0.lock().unwrap() == original_keys);
    assert!(!temp.path().join("restored").exists());
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
