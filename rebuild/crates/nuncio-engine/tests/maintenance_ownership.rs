#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    engine::{BackupInput, BackupUpload, Engine, EngineConfig, EngineError},
    secrets::{SecretError, SecretStore},
    store::{BackupArtifact, StoreError},
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
enum Held {
    Backup(BackupArtifact),
    Upload(BackupUpload),
    Input(BackupInput),
}
impl Held {
    fn release(self) {
        match self {
            Self::Backup(value) => drop(value),
            Self::Upload(value) => drop(value),
            Self::Input(value) => drop(value),
        }
    }
}
async fn check(kind: &str) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("profile");
    let secrets = Arc::new(Secrets::default());
    let config = || EngineConfig {
        directory: source.clone(),
        secrets: secrets.clone(),
    };
    let engine = Engine::open(config()).await.unwrap();
    let profile = engine.status().await.unwrap().profile_id;
    let keys = secrets.0.lock().unwrap().clone();
    let held = if kind == "backup" {
        Held::Backup(
            engine
                .create_backup(Zeroizing::new("synthetic maintenance lease phrase".into()))
                .await
                .unwrap(),
        )
    } else {
        let data = b"synthetic encrypted-input framing bytes";
        let upload = engine
            .begin_backup_upload(data.len() as u64, format!("{:x}", Sha256::digest(data)))
            .await
            .unwrap();
        if kind == "upload" {
            Held::Upload(upload)
        } else {
            Held::Input(
                upload
                    .append(0, data.to_vec())
                    .await
                    .unwrap()
                    .finish()
                    .await
                    .unwrap(),
            )
        }
    };
    engine.shutdown().await.unwrap();
    let opened = Engine::open(config()).await;
    let locked = matches!(opened, Err(EngineError::Storage(StoreError::Locked)));
    if let Ok(engine) = opened {
        engine.shutdown().await.unwrap();
    }
    held.release();
    let reopened = Engine::open(config()).await.unwrap();
    assert_eq!(reopened.status().await.unwrap().profile_id, profile);
    reopened.shutdown().await.unwrap();
    assert!(*secrets.0.lock().unwrap() == keys);
    assert!(
        locked,
        "profile reopened while {kind} still owned a maintenance resource"
    );
}
#[tokio::test]
async fn returned_backup_retains_profile_ownership_after_engine_shutdown() {
    check("backup").await;
}
#[tokio::test]
async fn incomplete_upload_retains_profile_ownership_after_engine_shutdown() {
    check("upload").await;
}
#[tokio::test]
async fn completed_upload_retains_profile_ownership_after_engine_shutdown() {
    check("input").await;
}
