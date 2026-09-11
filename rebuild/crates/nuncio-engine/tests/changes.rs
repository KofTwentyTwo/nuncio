#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{AccountRecord, Store, StoreError};
use zeroize::Zeroizing;

#[tokio::test]
async fn changes_replay_committed_revisions_and_explicitly_expire_slow_cursors() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x74; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let empty = store.changes_after(0, 100).await.unwrap();
    assert_eq!(empty.current_revision, 0);
    assert!(empty.changes.is_empty());
    assert!(matches!(
        store.changes_after(1, 100).await,
        Err(StoreError::ChangeHistoryExpired)
    ));
    let account = uuid::Uuid::new_v4().to_string();
    let mut reader = store.watch_changes(0).await.unwrap();
    let mut stalled = store.watch_changes(0).await.unwrap();
    store
        .add_account(AccountRecord {
            id: account.clone(),
            provider: "google".into(),
            address: "changes@example.test".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), reader.next())
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    let first = store.changes_after(0, 1).await.unwrap();
    assert_eq!(first.changes.len(), 1);
    assert_eq!(first.changes[0].revision, 1);
    assert_eq!(
        first.changes[0].account_id.as_deref(),
        Some(account.as_str())
    );
    // Invalid writes must not publish a change or advance the durable cursor.
    assert!(store
        .set_account_state(account.clone(), "invalid".into())
        .await
        .is_err());
    assert_eq!(
        store.changes_after(1, 100).await.unwrap().current_revision,
        1
    );
    for index in 0..10_000 {
        store
            .set_account_state(
                account.clone(),
                if index % 2 == 0 {
                    "needs_auth"
                } else {
                    "disconnected"
                }
                .into(),
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        store.changes_after(0, 100).await,
        Err(StoreError::ChangeHistoryExpired)
    ));
    let page = store.changes_after(1, 100).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), stalled.next())
            .await
            .unwrap(),
        Err(StoreError::ChangeHistoryExpired)
    ));
    assert_eq!(page.current_revision, 10_001);
    assert_eq!(page.changes.len(), 100);
    assert_eq!(page.changes[0].revision, 2);
    assert!(page.has_more);
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let last = store.changes_after(10_000, 100).await.unwrap();
    assert_eq!(last.changes[0].revision, 10_001);
    assert!(!last.has_more);
    assert!(matches!(
        store.changes_after(0, 100).await,
        Err(StoreError::ChangeHistoryExpired)
    ));
    store.close().await.unwrap();
}
