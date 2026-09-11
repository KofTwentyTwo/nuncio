#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    engine::{Engine, EngineConfig, EngineError},
    secrets::{SecretError, SecretStore},
    store::StoreError,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use zeroize::Zeroizing;
type ReleaseSignal = Arc<(Mutex<bool>, Condvar)>;
struct Barrier {
    entered: tokio::sync::oneshot::Sender<()>,
    release: ReleaseSignal,
}
#[derive(Default)]
struct Secrets {
    entries: Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>,
    barrier: Mutex<Option<Barrier>>,
}
impl SecretStore for Secrets {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        Ok(self.entries.lock().unwrap().get(name).cloned())
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        self.entries
            .lock()
            .unwrap()
            .insert(name.into(), Zeroizing::new(value.to_vec()));
        if name.ends_with("/profile/api") {
            let barrier = self.barrier.lock().unwrap().take();
            if let Some(Barrier { entered, release }) = barrier {
                let _ = entered.send(());
                let mut ready = release.0.lock().unwrap();
                while !*ready {
                    ready = release.1.wait(ready).unwrap();
                }
            }
        }
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.entries.lock().unwrap().remove(name);
        Ok(())
    }
}
struct Release(ReleaseSignal);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }
}
fn phrase() -> Zeroizing<String> {
    Zeroizing::new("synthetic detached restore ownership phrase".into())
}
#[tokio::test]
async fn cancelled_restore_keeps_source_profile_owned_until_blocking_work_finishes(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("original");
    let target = temp.path().join("restored");
    let secrets = Arc::new(Secrets::default());
    let config = || EngineConfig {
        directory: directory.clone(),
        secrets: secrets.clone(),
    };
    let engine = Arc::new(Engine::open(config()).await.unwrap());
    let original = engine.status().await.unwrap().profile_id;
    let original_keys = secrets.entries.lock().unwrap().clone();
    let backup = engine.create_backup(phrase()).await.unwrap();
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
    let input = upload.finish().await.unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let release = Release(Arc::new((Mutex::new(false), Condvar::new())));
    *secrets.barrier.lock().unwrap() = Some(Barrier {
        entered,
        release: release.0.clone(),
    });
    let worker = engine.clone();
    let caller = tokio::spawn(async move {
        worker
            .restore_backup(input, phrase(), "restored".into())
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let engine = Arc::into_inner(engine).unwrap();
    engine.shutdown().await.unwrap();
    let opened = Engine::open(config()).await;
    let retained = matches!(opened, Err(EngineError::Storage(StoreError::Locked)));
    if let Ok(engine) = opened {
        engine.shutdown().await.unwrap();
    }
    drop(release);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !target.exists() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let reopened = loop {
        match Engine::open(config()).await {
            Ok(engine) => break engine,
            Err(EngineError::Storage(StoreError::Locked)) => {
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => {
                return Err(error.into());
            }
        }
    };
    assert_eq!(reopened.status().await.unwrap().profile_id, original);
    reopened.shutdown().await.unwrap();
    let restored = Engine::open(EngineConfig {
        directory: target,
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    assert_ne!(restored.status().await.unwrap().profile_id, original);
    restored.shutdown().await.unwrap();
    let entries = secrets.entries.lock().unwrap();
    assert_eq!(entries.len(), 4);
    for (name, key) in original_keys {
        assert!(entries[&name] == key);
    }
    assert!(retained,"source profile became available while a cancelled caller's restore worker was still active");
    Ok(())
}
