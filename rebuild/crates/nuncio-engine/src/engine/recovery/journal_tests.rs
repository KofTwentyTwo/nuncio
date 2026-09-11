#![allow(clippy::unwrap_used)]
use super::journal::*;
use crate::{
    secrets::{SecretError, SecretStore},
    store::{DirectoryIdentity, OwnedDirectory, RestoreJob, Store},
};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
};
use zeroize::Zeroizing;
#[derive(Default)]
struct Secrets {
    keys: Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>,
    fail_delete: AtomicBool,
    delete_barrier: Mutex<Option<DeleteBarrier>>,
}
type ReleaseSignal = Arc<(Mutex<bool>, Condvar)>;
struct DeleteBarrier {
    entered: tokio::sync::oneshot::Sender<()>,
    release: ReleaseSignal,
}
struct Release(ReleaseSignal);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }
}
impl SecretStore for Secrets {
    fn get(&self, n: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        Ok(self.keys.lock().unwrap().get(n).cloned())
    }
    fn put(&self, n: &str, v: &[u8]) -> Result<(), SecretError> {
        self.keys
            .lock()
            .unwrap()
            .insert(n.into(), Zeroizing::new(v.to_vec()));
        Ok(())
    }
    fn delete(&self, n: &str) -> Result<(), SecretError> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(SecretError);
        }
        let barrier = self.delete_barrier.lock().unwrap().take();
        if let Some(barrier) = barrier {
            let _ = barrier.entered.send(());
            let mut ready = barrier.release.0.lock().unwrap();
            while !*ready {
                ready = barrier.release.1.wait(ready).unwrap();
            }
        }
        self.keys.lock().unwrap().remove(n);
        Ok(())
    }
}
struct Fixture {
    temp: tempfile::TempDir,
    owner: PathBuf,
    store: Store,
    job: RestoreJob,
    secrets: Arc<Secrets>,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let owner = temp.path().join("source");
        let store = Store::open(&owner, Zeroizing::new(vec![42; 32]))
            .await
            .unwrap();
        let stage = temp.path().join(".restore-owned");
        let upload = owner.join(".maintenance-owned");
        fs::create_dir(&stage).unwrap();
        fs::create_dir(&upload).unwrap();
        let job = RestoreJob {
            profile_id: crate::domain::identity::ProfileId::generate(),
            owner: DirectoryIdentity::at(&owner).unwrap(),
            parent: DirectoryIdentity::at(temp.path()).unwrap(),
            stage: OwnedDirectory {
                name: ".restore-owned".into(),
                identity: DirectoryIdentity::at(&stage).unwrap(),
            },
            upload: OwnedDirectory {
                name: ".maintenance-owned".into(),
                identity: DirectoryIdentity::at(&upload).unwrap(),
            },
            target: "restored".into(),
        };
        fs::write(stage.join("store.db"), b"owned encrypted stage canary").unwrap();
        fs::write(
            stage.join("profile.json"),
            serde_json::json!({"version":1,"id":job.profile_id}).to_string(),
        )
        .unwrap();
        fs::write(
            upload.join("upload.nuncio"),
            b"owned encrypted upload canary",
        )
        .unwrap();
        let secrets = Arc::new(Secrets::default());
        for suffix in ["database", "api"] {
            secrets
                .put(&format!("{}/profile/{suffix}", job.profile_id), &[77; 32])
                .unwrap();
        }
        secrets
            .put("unrelated/profile/database", &[99; 32])
            .unwrap();
        store.record_restore_job(job.clone()).await.unwrap();
        store
            .activate_restore_job(job.profile_id.to_string())
            .await
            .unwrap();
        Self {
            temp,
            owner,
            store,
            job,
            secrets,
        }
    }
    async fn recover(&self) -> Result<(), crate::engine::EngineError> {
        use fs4::fs_std::FileExt;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.owner.join("profile.lock"))
            .unwrap();
        assert!(lock.try_lock_exclusive().unwrap());
        recover(
            &self.store,
            self.owner.clone(),
            crate::domain::identity::ProfileId::generate(),
            self.secrets.clone(),
            Arc::new(lock),
        )
        .await
    }
    fn stage(&self) -> PathBuf {
        self.temp.path().join(&self.job.stage.name)
    }
    fn target(&self) -> PathBuf {
        self.temp.path().join(&self.job.target)
    }
    fn keys(&self) -> BTreeMap<String, Zeroizing<Vec<u8>>> {
        self.secrets.keys.lock().unwrap().clone()
    }
}
#[tokio::test]
async fn uncertain_paths_preserve_keys_files_and_retryable_cleanup_authority() {
    for scenario in [
        "stage-replaced",
        "stage-symlink",
        "stage-missing",
        "target-wrong-id",
        "owner-copied",
        "owner-symlink",
        "unexpected-stage-entry",
        "upload-replaced",
    ] {
        let mut f = Fixture::new().await;
        let keys = f.keys();
        match scenario {
            "stage-replaced" => {
                fs::rename(f.stage(), f.temp.path().join("moved")).unwrap();
                fs::create_dir(f.stage()).unwrap();
                fs::write(f.stage().join("store.db"), b"unrelated replacement").unwrap();
            }
            "stage-symlink" => {
                fs::rename(f.stage(), f.temp.path().join("moved")).unwrap();
                std::os::unix::fs::symlink(f.temp.path().join("moved"), f.stage()).unwrap();
            }
            "stage-missing" => {
                fs::rename(f.stage(), f.temp.path().join("moved")).unwrap();
            }
            "target-wrong-id" => {
                fs::rename(f.stage(), f.target()).unwrap();
                fs::write(f.target().join("profile.json"),serde_json::json!({"version":1,"id":crate::domain::identity::ProfileId::generate()}).to_string()).unwrap();
            }
            "owner-copied" => {
                f.owner = f.temp.path().join("copied");
                fs::create_dir(&f.owner).unwrap();
            }
            "owner-symlink" => {
                let moved = f.temp.path().join("moved-owner");
                fs::rename(&f.owner, &moved).unwrap();
                std::os::unix::fs::symlink(&moved, &f.owner).unwrap();
            }
            "unexpected-stage-entry" => {
                fs::write(f.stage().join("unrelated"), b"preserve this").unwrap();
            }
            "upload-replaced" => {
                let upload = f.owner.join(&f.job.upload.name);
                fs::rename(&upload, f.owner.join("moved-upload")).unwrap();
                fs::create_dir(&upload).unwrap();
                fs::write(upload.join("upload.nuncio"), b"unrelated replacement").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(f.recover().await.is_err(), "{scenario}");
        assert!(f.keys() == keys, "keys changed for {scenario}");
        assert_eq!(f.store.restore_jobs().await.unwrap().len(), 1);
        assert!(f.recover().await.is_err(), "second recovery: {scenario}");
        assert!(f.keys() == keys, "second recovery changed keys: {scenario}");
        f.store.close().await.unwrap();
    }
}
#[tokio::test]
async fn failed_key_cleanup_is_retryable_and_activated_target_retains_keys() {
    let f = Fixture::new().await;
    let keys = f.keys();
    f.secrets.fail_delete.store(true, Ordering::SeqCst);
    assert!(f.recover().await.is_err());
    assert!(f.keys() == keys);
    assert!(f.stage().join("store.db").exists());
    assert_eq!(f.store.restore_jobs().await.unwrap().len(), 1);
    f.secrets.fail_delete.store(false, Ordering::SeqCst);
    f.recover().await.unwrap();
    assert!(!f.stage().exists());
    assert!(!f.owner.join(&f.job.upload.name).exists());
    assert_eq!(f.keys().len(), 1);
    assert!(f.store.restore_jobs().await.unwrap().is_empty());
    f.recover().await.unwrap();
    f.store.close().await.unwrap();
    let f = Fixture::new().await;
    let keys = f.keys();
    fs::rename(f.stage(), f.target()).unwrap();
    f.recover().await.unwrap();
    assert!(f.keys() == keys);
    assert_eq!(
        fs::read(f.target().join("store.db")).unwrap(),
        b"owned encrypted stage canary"
    );
    assert!(!f.owner.join(&f.job.upload.name).exists());
    assert!(f.store.restore_jobs().await.unwrap().is_empty());
    f.recover().await.unwrap();
    f.store.close().await.unwrap();
}
#[tokio::test]
async fn backup_excludes_cleanup_authority_while_source_retains_it() {
    let f = Fixture::new().await;
    let keys = f.keys();
    let phrase: Zeroizing<String> = Zeroizing::new("synthetic cleanup backup passphrase".into());
    let backup = f.store.create_backup(phrase.clone(), 10).await.unwrap();
    let c = rusqlite::Connection::open(backup.path()).unwrap();
    c.pragma_update(None, "key", phrase.as_str()).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM restore_cleanup_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(f.store.restore_jobs().await.unwrap().len(), 1);
    assert!(f.keys() == keys);
    c.close().unwrap();
    drop(backup);
    f.store.close().await.unwrap();
}

#[tokio::test]
async fn journal_accepts_parent_aliases_without_accepting_another_owners_upload() {
    let f = Fixture::new().await;
    let alias = f.temp.path().join("parent-alias");
    std::os::unix::fs::symlink(f.temp.path(), &alias).unwrap();
    let source = alias
        .join("source")
        .join(&f.job.upload.name)
        .join("upload.nuncio");
    let other = f.temp.path().join("other-owner");
    fs::create_dir(&other).unwrap();
    let other_source = other.join(".maintenance-other/upload.nuncio");
    let journal = Journal {
        store: f.store.clone(),
        owner: canonical_owner(&f.owner).unwrap(),
        runtime: tokio::runtime::Handle::current(),
    };
    let id = crate::domain::identity::ProfileId::generate();
    let stage = f.stage();
    let target = f.target();
    let parent = f.job.parent;
    let identity = f.job.stage.identity;
    tokio::task::spawn_blocking(move || {
        journal
            .record(id, &source, &target, &stage, parent, identity)
            .unwrap();
        assert!(matches!(
            journal.record(
                crate::domain::identity::ProfileId::generate(),
                &other_source,
                &target,
                &stage,
                parent,
                identity,
            ),
            Err(crate::store::StoreError::InvalidPath)
        ));
    })
    .await
    .unwrap();
    let jobs = f.store.restore_jobs().await.unwrap();
    assert_eq!(jobs.len(), 2);
    let (recorded, activating) = jobs.iter().find(|(job, _)| job.profile_id == id).unwrap();
    assert!(!activating);
    assert_eq!(recorded.owner, f.job.owner);
    assert_eq!(recorded.upload.identity, f.job.upload.identity);
    f.store.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_startup_retains_profile_ownership_until_cleanup_worker_finishes() {
    use crate::{
        domain::identity::ProfileId,
        engine::{Engine, EngineConfig, EngineError},
    };
    use std::time::Duration;
    let f = Fixture::new().await;
    let source_id = ProfileId::generate();
    fs::write(
        f.owner.join("profile.json"),
        serde_json::json!({"version":1,"id":source_id}).to_string(),
    )
    .unwrap();
    f.secrets
        .put(&format!("{source_id}/profile/database"), &[42; 32])
        .unwrap();
    f.secrets
        .put(&format!("{source_id}/profile/api"), &[55; 32])
        .unwrap();
    let original = f
        .keys()
        .into_iter()
        .filter(|(n, _)| !n.starts_with(&f.job.profile_id.to_string()))
        .collect::<BTreeMap<_, _>>();
    let manifest = fs::read(f.owner.join("profile.json")).unwrap();
    f.store.clone().close().await.unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let release = Release(Arc::new((Mutex::new(false), Condvar::new())));
    *f.secrets.delete_barrier.lock().unwrap() = Some(DeleteBarrier {
        entered,
        release: release.0.clone(),
    });
    let config = || EngineConfig {
        directory: f.owner.clone(),
        secrets: f.secrets.clone(),
    };
    let caller = tokio::spawn(Engine::open(config()));
    tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.err().unwrap().is_cancelled());
    let opened = tokio::time::timeout(Duration::from_secs(10), Engine::open(config()))
        .await
        .unwrap();
    let retained = matches!(
        opened,
        Err(EngineError::Storage(crate::store::StoreError::Locked))
    );
    if let Ok(engine) = opened {
        engine.shutdown().await.unwrap();
    }
    drop(release);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let engine = loop {
        match Engine::open(config()).await {
            Ok(engine) => break engine,
            Err(error) => {
                assert!(
                    matches!(
                        error,
                        EngineError::Storage(crate::store::StoreError::Locked)
                    ),
                    "startup after cleanup failed: {error}"
                );
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    };
    assert_eq!(
        engine.status().await.unwrap().profile_id,
        source_id.to_string()
    );
    engine.shutdown().await.unwrap();
    assert!(f.keys() == original);
    assert_eq!(fs::read(f.owner.join("profile.json")).unwrap(), manifest);
    assert!(!f.stage().exists());
    assert!(!f.owner.join(&f.job.upload.name).exists());
    assert!(
        retained,
        "source reopened while a cancelled startup's cleanup worker was still active"
    );
}
