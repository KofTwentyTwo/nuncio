#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    engine::{Engine, EngineConfig},
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
fn phrase() -> Zeroizing<String> {
    Zeroizing::new("synthetic maintenance recovery phrase".into())
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[tokio::test]
async fn backup_artifact_and_upload_hold_one_maintenance_slot_until_finished_or_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::open(EngineConfig {
        directory: temp.path().join("original"),
        secrets: Arc::new(Secrets::default()),
    })
    .await
    .unwrap();
    let backup = engine.create_backup(phrase()).await.unwrap();
    let bytes = std::fs::read(backup.path()).unwrap();
    let info = backup.inspection().clone();
    assert!(matches!(
        engine.create_backup(phrase()).await,
        Err(StoreError::Busy)
    ));
    assert!(matches!(
        engine
            .begin_backup_upload(bytes.len() as u64, sha(&bytes))
            .await,
        Err(StoreError::Busy)
    ));
    let path = backup.path().to_owned();
    drop(backup);
    assert!(!path.exists());
    let mut upload = engine
        .begin_backup_upload(bytes.len() as u64, sha(&bytes))
        .await
        .unwrap();
    assert!(matches!(
        engine.create_backup(phrase()).await,
        Err(StoreError::Busy)
    ));
    for (i, part) in bytes.chunks(131071).enumerate() {
        upload = upload
            .append((i * 131071) as u64, part.to_vec())
            .await
            .unwrap();
    }
    let input = upload.finish().await.unwrap();
    assert!(matches!(
        engine.create_backup(phrase()).await,
        Err(StoreError::Busy)
    ));
    let inspected = engine.inspect_backup(input, phrase()).await.unwrap();
    assert_eq!(inspected, info);
    let next = engine.create_backup(phrase()).await.unwrap();
    drop(next);
    let upload = engine
        .begin_backup_upload(bytes.len() as u64, sha(&bytes))
        .await
        .unwrap();
    drop(upload);
    assert!(!std::fs::read_dir(temp.path().join("original"))
        .unwrap()
        .any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".maintenance-")));
    let next = engine.create_backup(phrase()).await.unwrap();
    drop(next);
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn upload_rejects_bad_framing_hashes_lengths_and_incomplete_files_without_leaking_its_slot() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::open(EngineConfig {
        directory: temp.path().into(),
        secrets: Arc::new(Secrets::default()),
    })
    .await
    .unwrap();
    for (length, hash) in [
        (0, sha(b"")),
        (1, "bad".into()),
        (1, "A".repeat(64)),
        ((1_u64 << 40) + 1, sha(b"a")),
    ] {
        assert!(engine.begin_backup_upload(length, hash).await.is_err());
    }
    for case in [
        "offset",
        "empty",
        "oversize",
        "past-length",
        "short",
        "hash",
    ] {
        let upload = engine
            .begin_backup_upload(300000, sha(&vec![42; 300000]))
            .await
            .unwrap();
        let result = match case {
            "offset" => upload.append(1, vec![42]).await.map(|_| ()),
            "empty" => upload.append(0, vec![]).await.map(|_| ()),
            "oversize" => upload.append(0, vec![42; 262145]).await.map(|_| ()),
            "past-length" => upload
                .append(0, vec![42; 200000])
                .await
                .unwrap()
                .append(200000, vec![42; 100001])
                .await
                .map(|_| ()),
            "short" => upload
                .append(0, vec![42; 1])
                .await
                .unwrap()
                .finish()
                .await
                .map(|_| ()),
            _ => upload
                .append(0, vec![43; 200000])
                .await
                .unwrap()
                .append(200000, vec![43; 100000])
                .await
                .unwrap()
                .finish()
                .await
                .map(|_| ()),
        };
        assert!(
            matches!(result, Err(StoreError::InvalidInput)),
            "accepted {case}"
        );
        assert!(!std::fs::read_dir(temp.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".maintenance-")));
        let next = engine.create_backup(phrase()).await.unwrap();
        drop(next);
    }
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn engine_restores_uploaded_backup_only_to_a_new_named_sibling_profile() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(Secrets::default());
    let engine = Engine::open(EngineConfig {
        directory: temp.path().join("source"),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let backup = engine.create_backup(phrase()).await.unwrap();
    let bytes = std::fs::read(backup.path()).unwrap();
    drop(backup);
    for name in ["", ".", "..", "nested/name", "../elsewhere", "source"] {
        let mut upload = engine
            .begin_backup_upload(bytes.len() as u64, sha(&bytes))
            .await
            .unwrap();
        for (i, part) in bytes.chunks(262144).enumerate() {
            upload = upload
                .append((i * 262144) as u64, part.to_vec())
                .await
                .unwrap();
        }
        assert!(engine
            .restore_backup(upload.finish().await.unwrap(), phrase(), name.into())
            .await
            .is_err());
        assert_eq!(secrets.0.lock().unwrap().len(), 2);
    }
    let mut upload = engine
        .begin_backup_upload(bytes.len() as u64, sha(&bytes))
        .await
        .unwrap();
    for (i, part) in bytes.chunks(262144).enumerate() {
        upload = upload
            .append((i * 262144) as u64, part.to_vec())
            .await
            .unwrap();
    }
    let report = engine
        .restore_backup(upload.finish().await.unwrap(), phrase(), "recovered".into())
        .await
        .unwrap();
    assert_eq!(
        report.directory,
        std::fs::canonicalize(temp.path())
            .unwrap()
            .join("recovered")
    );
    let restored = Engine::open(EngineConfig {
        directory: report.directory,
        secrets,
    })
    .await
    .unwrap();
    assert_eq!(
        restored.status().await.unwrap().profile_id,
        report.profile_id
    );
    assert!(restored.authorization() != engine.authorization());
    restored.shutdown().await.unwrap();
    engine.shutdown().await.unwrap();
}

#[derive(Default)]
struct BlockingSecrets {
    inner: Secrets,
    block: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    released: Mutex<bool>,
    wake: std::sync::Condvar,
}
impl SecretStore for BlockingSecrets {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        self.inner.get(name)
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        self.inner.put(name, value)?;
        if self.block.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.entered.notify_one();
            let (released, timeout) = self
                .wake
                .wait_timeout_while(
                    self.released.lock().unwrap(),
                    std::time::Duration::from_secs(10),
                    |value| !*value,
                )
                .unwrap();
            if timeout.timed_out() || !*released {
                return Err(SecretError);
            }
        }
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.inner.delete(name)
    }
}
struct Unblock(Arc<BlockingSecrets>);
impl Drop for Unblock {
    fn drop(&mut self) {
        if let Ok(mut released) = self.0.released.lock() {
            *released = true;
            self.0.wake.notify_all();
        }
    }
}

#[tokio::test]
async fn cancelled_restore_caller_does_not_release_admission_while_owned_blocking_work_is_live() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(BlockingSecrets::default());
    let engine = Arc::new(
        Engine::open(EngineConfig {
            directory: temp.path().join("source"),
            secrets: secrets.clone(),
        })
        .await
        .unwrap(),
    );
    let backup = engine.create_backup(phrase()).await.unwrap();
    let bytes = std::fs::read(backup.path()).unwrap();
    drop(backup);
    let mut upload = engine
        .begin_backup_upload(bytes.len() as u64, sha(&bytes))
        .await
        .unwrap();
    for (i, part) in bytes.chunks(262144).enumerate() {
        upload = upload
            .append((i * 262144) as u64, part.to_vec())
            .await
            .unwrap();
    }
    let input = upload.finish().await.unwrap();
    secrets
        .block
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let unblock = Unblock(secrets.clone());
    let worker_engine = engine.clone();
    let task = tokio::spawn(async move {
        worker_engine
            .restore_backup(input, phrase(), "after-cancel".into())
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        secrets.entered.notified(),
    )
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(matches!(
        engine.begin_backup_upload(1, sha(b"a")).await,
        Err(StoreError::Busy)
    ));
    assert!(matches!(
        engine.create_backup(phrase()).await,
        Err(StoreError::Busy)
    ));
    drop(unblock);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match engine.begin_backup_upload(1, sha(b"a")).await {
                Ok(upload) => {
                    drop(upload);
                    break;
                }
                Err(error) => assert!(matches!(error, StoreError::Busy)),
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let restored = Engine::open(EngineConfig {
        directory: temp.path().join("after-cancel"),
        secrets,
    })
    .await
    .unwrap();
    assert!(restored.authorization() != engine.authorization());
    restored.shutdown().await.unwrap();
    assert!(!std::fs::read_dir(temp.path().join("source"))
        .unwrap()
        .any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".maintenance-")));
    Arc::try_unwrap(engine)
        .ok()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}
