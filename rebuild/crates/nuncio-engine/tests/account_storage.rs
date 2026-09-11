#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{ConnectedAccount, Store};
use zeroize::Zeroizing;

#[tokio::test]
async fn credential_intent_commit_and_disconnect_are_durable_without_erasing_account() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x51; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let first = format!("profile/account/{id}/one");
    store.prepare_credential(first.clone()).await.unwrap();
    assert_eq!(
        store.credential_cleanup().await.unwrap(),
        vec![first.clone()]
    );
    store
        .connect_google(ConnectedAccount {
            id: id.clone(),
            subject: "100001".into(),
            address: "alpha@example.test".into(),
            credential_ref: first.clone(),
        })
        .await
        .unwrap();
    assert!(store.credential_cleanup().await.unwrap().is_empty());
    let second = format!("profile/account/{id}/two");
    store.prepare_credential(second.clone()).await.unwrap();
    store
        .connect_google(ConnectedAccount {
            id: id.clone(),
            subject: "different-user".into(),
            address: "alpha@example.test".into(),
            credential_ref: second.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        store
            .account(id.clone())
            .await
            .unwrap()
            .unwrap()
            .credential_ref,
        Some(first.clone())
    );
    store
        .connect_google(ConnectedAccount {
            id: id.clone(),
            subject: "100001".into(),
            address: "renamed@example.test".into(),
            credential_ref: second.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        store.credential_cleanup().await.unwrap(),
        vec![first.clone()]
    );
    store
        .set_account_state(id.clone(), "disconnected".into())
        .await
        .unwrap();
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let account = store.account(id.clone()).await.unwrap().unwrap();
    assert_eq!(account.state, "disconnected");
    assert_eq!(account.account.address, "renamed@example.test");
    assert_eq!(account.subject.as_deref(), Some("100001"));
    assert!(account.credential_ref.is_none());
    assert_eq!(store.credential_cleanup().await.unwrap().len(), 2);
    store.finish_credential_cleanup(first).await.unwrap();
    store.finish_credential_cleanup(second).await.unwrap();
    assert_eq!(store.status().await.unwrap().account_count, 1);
    store.close().await.unwrap();
}
