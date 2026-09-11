#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    engine::{Engine, EngineConfig, EngineError},
    secrets::{SecretError, SecretStore},
    store::StoreError,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;
#[derive(Default)]
struct Secrets(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);
impl SecretStore for Secrets {
    fn get(&self, n: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        Ok(self.0.lock().unwrap().get(n).cloned())
    }
    fn put(&self, n: &str, v: &[u8]) -> Result<(), SecretError> {
        self.0
            .lock()
            .unwrap()
            .insert(n.into(), Zeroizing::new(v.to_vec()));
        Ok(())
    }
    fn delete(&self, n: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().remove(n);
        Ok(())
    }
}
fn phrase() -> Zeroizing<String> {
    Zeroizing::new("synthetic restore cleanup failure phrase".into())
}
#[tokio::test]
async fn activated_restore_reports_upload_cleanup_failure_and_retries_without_deleting_its_keys() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let secrets = Arc::new(Secrets::default());
    let config = || EngineConfig {
        directory: source.clone(),
        secrets: secrets.clone(),
    };
    let engine = Engine::open(config()).await.unwrap();
    let original = secrets.0.lock().unwrap().clone();
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
    let directory = std::fs::read_dir(&source)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".maintenance-")
        })
        .unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = engine
        .restore_backup(input, phrase(), "restored".into())
        .await;
    // Repair test-owned permissions even if the assertion fails.
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        matches!(
            result,
            Err(EngineError::Storage(StoreError::RestoreActivationUncertain))
        ),
        "cleanup failure must not report completed restore"
    );
    assert!(directory.join("upload.nuncio").exists());
    let target = temp.path().join("restored");
    assert!(target.join("store.db").exists());
    let keys = secrets.0.lock().unwrap().clone();
    assert_eq!(keys.len(), 4);
    engine.shutdown().await.unwrap();
    let reopened = Engine::open(config()).await.unwrap();
    assert!(!directory.exists());
    assert!(*secrets.0.lock().unwrap() == keys);
    reopened.shutdown().await.unwrap();
    let restored = Engine::open(EngineConfig {
        directory: target,
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    restored.shutdown().await.unwrap();
    for (n, v) in original {
        assert!(secrets.0.lock().unwrap()[&n] == v);
    }
}
