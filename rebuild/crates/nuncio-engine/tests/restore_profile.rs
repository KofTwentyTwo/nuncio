#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    engine::{Engine, EngineConfig, EngineError},
    secrets::{SecretError, SecretStore},
    store::Store,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use zeroize::Zeroizing;

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
fn phrase() -> Zeroizing<String> {
    Zeroizing::new("synthetic restore profile recovery phrase".into())
}

#[tokio::test]
async fn restored_profile_opens_with_new_identity_and_keys_while_original_remains_usable() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original");
    let restored = temp.path().join("restored");
    let secrets = Arc::new(MemorySecrets::default());
    let engine = Engine::open(EngineConfig {
        directory: original.clone(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let original_id = engine.status().await.unwrap().profile_id;
    let original_authorization = engine.authorization();
    engine.shutdown().await.unwrap();
    let original_entries = secrets.0.lock().unwrap().clone();
    let key = secrets
        .get(&format!("{original_id}/profile/database"))
        .unwrap()
        .unwrap();
    let store = Store::open(&original, key).await.unwrap();
    let backup = store.create_backup(phrase(), 1000).await.unwrap();
    store.close().await.unwrap();
    let manifest_before = std::fs::read(original.join("profile.json")).unwrap();
    let backup_before = std::fs::read(backup.path()).unwrap();
    let report = Engine::restore_profile(
        EngineConfig {
            directory: restored.clone(),
            secrets: secrets.clone(),
        },
        backup.path().into(),
        phrase(),
    )
    .await
    .unwrap();
    assert_ne!(report.profile_id, original_id);
    assert_eq!(report.directory, restored);
    assert_eq!(report.restore.schema_version, 23);
    let engine = Engine::open(EngineConfig {
        directory: restored.clone(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    assert_eq!(engine.status().await.unwrap().profile_id, report.profile_id);
    assert!(engine.authorization() != original_authorization);
    engine.shutdown().await.unwrap();
    {
        let entries = secrets.0.lock().unwrap();
        assert_eq!(entries.len(), 4);
        for (name, value) in &original_entries {
            assert!(entries[name] == *value, "changed original key");
        }
        let values = entries.values().collect::<Vec<_>>();
        for i in 0..values.len() {
            for j in 0..i {
                assert!(values[i] != values[j], "reused profile key");
            }
        }
    }
    assert_eq!(
        std::fs::read(original.join("profile.json")).unwrap(),
        manifest_before
    );
    assert_eq!(std::fs::read(backup.path()).unwrap(), backup_before);
    let original_engine = Engine::open(EngineConfig {
        directory: original,
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    assert_eq!(
        original_engine.status().await.unwrap().profile_id,
        original_id
    );
    assert!(original_engine.authorization() == original_authorization);
    original_engine.shutdown().await.unwrap();
}

struct FailingSecrets {
    inner: Arc<MemorySecrets>,
    writes: AtomicUsize,
    fail_write: usize,
    fail_delete: bool,
    create_target: Option<std::path::PathBuf>,
}
impl SecretStore for FailingSecrets {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        self.inner.get(name)
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        self.inner.put(name, value)?;
        let ordinal = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
        if ordinal == 1 {
            if let Some(target) = &self.create_target {
                std::fs::create_dir(target).unwrap();
                std::fs::write(target.join("canary"), b"concurrent original").unwrap();
            }
        }
        if ordinal == self.fail_write {
            Err(SecretError)
        } else {
            Ok(())
        }
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        if self.fail_delete {
            Err(SecretError)
        } else {
            self.inner.delete(name)
        }
    }
}

#[tokio::test]
async fn restore_cleans_new_keys_after_partial_keystore_failure_and_reports_incomplete_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("source"), Zeroizing::new(vec![0x45; 32]))
        .await
        .unwrap();
    let backup = store.create_backup(phrase(), 1000).await.unwrap();
    let original_bytes = std::fs::read(backup.path()).unwrap();
    for (write, delete) in [(1, false), (2, false), (2, true)] {
        let inner = Arc::new(MemorySecrets::default());
        inner.put("unrelated-original-key", &[0x66; 32]).unwrap();
        let secrets = Arc::new(FailingSecrets {
            inner: inner.clone(),
            writes: AtomicUsize::new(0),
            fail_write: write,
            fail_delete: delete,
            create_target: None,
        });
        let target = temp.path().join(format!("restore-{write}-{delete}"));
        let result = Engine::restore_profile(
            EngineConfig {
                directory: target.clone(),
                secrets,
            },
            backup.path().into(),
            phrase(),
        )
        .await;
        assert!(result.is_err());
        assert!(!target.exists());
        let entries = inner.0.lock().unwrap();
        assert!(entries["unrelated-original-key"].as_slice() == [0x66; 32]);
        if delete {
            let Err(EngineError::RestoreCleanupIncomplete { profile_id }) = result else {
                unreachable!("cleanup failure must name the newly owned profile")
            };
            assert_eq!(entries.len(), 3);
            assert!(entries.contains_key(&format!("{profile_id}/profile/database")));
            assert!(entries.contains_key(&format!("{profile_id}/profile/api")));
        } else {
            assert_eq!(entries.len(), 1);
        }
        assert_eq!(std::fs::read(backup.path()).unwrap(), original_bytes);
        assert!(!std::fs::read_dir(temp.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".restore-")));
    }
    store.close().await.unwrap();
}

#[tokio::test]
async fn concurrent_target_creation_prevents_activation_and_removes_only_new_profile_keys() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("source"), Zeroizing::new(vec![0x57; 32]))
        .await
        .unwrap();
    let backup = store.create_backup(phrase(), 1000).await.unwrap();
    let before = std::fs::read(backup.path()).unwrap();
    let inner = Arc::new(MemorySecrets::default());
    inner.put("unrelated-original-key", &[0x67; 32]).unwrap();
    let target = temp.path().join("concurrent-target");
    let secrets = Arc::new(FailingSecrets {
        inner: inner.clone(),
        writes: AtomicUsize::new(0),
        fail_write: 0,
        fail_delete: false,
        create_target: Some(target.clone()),
    });
    let result = Engine::restore_profile(
        EngineConfig {
            directory: target.clone(),
            secrets: secrets.clone(),
        },
        backup.path().into(),
        phrase(),
    )
    .await;
    assert!(matches!(
        result,
        Err(EngineError::Storage(nuncio_engine::store::StoreError::Io(
            std::io::ErrorKind::AlreadyExists
        )))
    ));
    assert_eq!(secrets.writes.load(Ordering::SeqCst), 2);
    assert_eq!(inner.0.lock().unwrap().len(), 1);
    assert!(
        inner
            .get("unrelated-original-key")
            .unwrap()
            .unwrap()
            .as_slice()
            == [0x67; 32]
    );
    assert_eq!(
        std::fs::read(target.join("canary")).unwrap(),
        b"concurrent original"
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
    assert_eq!(std::fs::read(backup.path()).unwrap(), before);
    assert!(!std::fs::read_dir(temp.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".restore-")));
    assert!(Engine::restore_profile(
        EngineConfig {
            directory: target.clone(),
            secrets: secrets.clone()
        },
        backup.path().into(),
        phrase()
    )
    .await
    .is_err());
    assert_eq!(secrets.writes.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read(target.join("canary")).unwrap(),
        b"concurrent original"
    );
    store.close().await.unwrap();
}

struct ReplacedParentSecrets {
    inner: Arc<MemorySecrets>,
    parent: std::path::PathBuf,
    relocated: std::path::PathBuf,
    writes: AtomicUsize,
}
impl SecretStore for ReplacedParentSecrets {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        self.inner.get(name)
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        self.inner.put(name, value)?;
        if self.writes.fetch_add(1, Ordering::SeqCst) == 0 {
            let entry = std::fs::read_dir(&self.parent)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .file_name();
            std::fs::rename(&self.parent, &self.relocated).unwrap();
            std::fs::create_dir(&self.parent).unwrap();
            std::fs::create_dir(self.parent.join(&entry)).unwrap();
            std::fs::write(
                self.parent.join(entry).join("unrelated"),
                b"replacement directory canary",
            )
            .unwrap();
        }
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.inner.delete(name)
    }
}
#[tokio::test]
async fn parent_replacement_during_key_creation_preserves_unrelated_files_and_rolls_back_only_new_keys(
) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("source"), Zeroizing::new(vec![0x5a; 32]))
        .await
        .unwrap();
    let backup = store.create_backup(phrase(), 1000).await.unwrap();
    let original = std::fs::read(backup.path()).unwrap();
    let parent = temp.path().join("parent");
    std::fs::create_dir(&parent).unwrap();
    let relocated = temp.path().join("relocated");
    let inner = Arc::new(MemorySecrets::default());
    inner.put("unrelated-original-key", &[0x63; 32]).unwrap();
    let secrets = Arc::new(ReplacedParentSecrets {
        inner: inner.clone(),
        parent: parent.clone(),
        relocated: relocated.clone(),
        writes: AtomicUsize::new(0),
    });
    let result = Engine::restore_profile(
        EngineConfig {
            directory: parent.join("restored"),
            secrets: secrets.clone(),
        },
        backup.path().into(),
        phrase(),
    )
    .await;
    assert!(matches!(
        result,
        Err(EngineError::Storage(
            nuncio_engine::store::StoreError::InvalidPath
        ))
    ));
    assert_eq!(secrets.writes.load(Ordering::SeqCst), 2);
    assert_eq!(inner.0.lock().unwrap().len(), 1);
    assert_eq!(
        inner
            .get("unrelated-original-key")
            .unwrap()
            .unwrap()
            .as_slice(),
        &[0x63; 32]
    );
    assert!(!parent.join("restored").exists());
    assert!(!relocated.join("restored").exists());
    let substitute = std::fs::read_dir(&parent)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::read(substitute.join("unrelated")).unwrap(),
        b"replacement directory canary"
    );
    assert_eq!(std::fs::read_dir(substitute).unwrap().count(), 1);
    let old_stage = std::fs::read_dir(relocated)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(old_stage.join("store.db").is_file());
    assert!(old_stage.join("profile.json").is_file());
    assert_eq!(std::fs::read(backup.path()).unwrap(), original);
    store.close().await.unwrap();
}
