#![allow(clippy::unwrap_used, clippy::expect_used)]

use nuncio_engine::store::{AccountRecord, Store, StoreError};
use zeroize::Zeroizing;

#[tokio::test]
async fn encrypted_store_can_be_created_and_reopened() {
    let directory = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x41; 32]);
    let store = Store::open(directory.path(), key.clone()).await;
    assert!(store.is_ok(), "valid new encrypted store must open");
    store.unwrap().close().await.unwrap();
    Store::open(directory.path(), key)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}

#[tokio::test]
async fn committed_accounts_survive_restart_without_plaintext_in_files() {
    let directory = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x42; 32]);
    let store = Store::open(directory.path(), key.clone()).await.unwrap();
    store
        .add_account(AccountRecord {
            id: "0f703c3b-a3ec-4298-9f5c-8e4ec141684c".into(),
            provider: "google".into(),
            address: "storage-canary@example.invalid".into(),
        })
        .await
        .unwrap();
    let status = store.status().await.unwrap();
    assert_eq!(status.account_count, 1);
    assert_eq!(status.revision, 1);
    for entry in std::fs::read_dir(directory.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!bytes.windows(14).any(|value| value == b"storage-canary"));
    }
    store.close().await.unwrap();
    let store = Store::open(directory.path(), key).await.unwrap();
    assert_eq!(
        store.accounts().await.unwrap()[0].address,
        "storage-canary@example.invalid"
    );
    store.close().await.unwrap();
    let plain = rusqlite::Connection::open(directory.path().join("store.db")).unwrap();
    assert!(plain
        .query_row("SELECT count(*) FROM accounts", [], |row| row
            .get::<_, i64>(0))
        .is_err());
}

#[tokio::test]
async fn wrong_key_and_corruption_do_not_replace_original_database() {
    let directory = tempfile::tempdir().unwrap();
    Store::open(directory.path(), Zeroizing::new(vec![0x43; 32]))
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let path = directory.path().join("store.db");
    let original = std::fs::read(&path).unwrap();
    assert!(
        Store::open(directory.path(), Zeroizing::new(vec![0x44; 32]))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::write(&path, b"deliberately broken database").unwrap();
    assert!(
        Store::open(directory.path(), Zeroizing::new(vec![0x43; 32]))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"deliberately broken database"
    );
}

#[tokio::test]
async fn second_owner_is_rejected_until_first_owner_closes() {
    let directory = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x45; 32]);
    let first = Store::open(directory.path(), key.clone()).await.unwrap();
    let second = Store::open(directory.path(), key.clone()).await;
    assert!(matches!(second, Err(StoreError::Locked)));
    first.close().await.unwrap();
    Store::open(directory.path(), key)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}

#[tokio::test]
async fn invalid_key_size_is_rejected_before_creating_database() {
    let directory = tempfile::tempdir().unwrap();
    assert!(Store::open(directory.path(), Zeroizing::new(vec![0; 31]))
        .await
        .is_err());
    assert!(!directory.path().join("store.db").exists());
}

#[tokio::test]
async fn newer_schema_is_rejected_without_changing_its_journal_or_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x46; 32]);
    Store::open(directory.path(), key.clone())
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let path = directory.path().join("store.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode(key.as_slice())))
        .unwrap();
    connection
        .pragma_update(None, "journal_mode", "DELETE")
        .unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    connection.close().unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(matches!(
        Store::open(directory.path(), key).await,
        Err(StoreError::FutureSchema)
    ));
    assert!(
        std::fs::read(&path).unwrap() == original,
        "unsupported schema must retain exact database bytes"
    );
}
