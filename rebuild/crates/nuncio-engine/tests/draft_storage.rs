#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::drafts::DraftContent,
    store::{AccountRecord, SaveDraft, Store, StoreError},
};
use zeroize::Zeroizing;

fn input(account: &str) -> SaveDraft {
    SaveDraft {
        account_id: account.into(),
        id: None,
        expected_version: None,
        content: DraftContent {
            subject: "private-draft-canary".into(),
            text: Some("Private original body".into()),
            ..Default::default()
        },
    }
}
#[tokio::test]
async fn drafts_are_encrypted_scoped_versioned_and_replay_changes_without_lost_edits() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x7b; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    for id in [&account, &other] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "draft@example.test".into(),
            })
            .await
            .unwrap();
    }
    let saved = store.save_draft(input(&account), 1000).await.unwrap();
    assert_eq!(saved.version, 1);
    assert_eq!(saved.created_at_ms, 1000);
    let before = store.status().await.unwrap().revision;
    let mut edit = input(&account);
    edit.id = Some(saved.id.clone());
    edit.expected_version = Some(1);
    edit.content.subject = "First edit".into();
    let (one, two) = tokio::join!(
        store.save_draft(edit.clone(), 1001),
        store.save_draft(edit, 1001)
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert!(
        matches!(one, Err(StoreError::VersionConflict))
            || matches!(two, Err(StoreError::VersionConflict))
    );
    assert_eq!(store.status().await.unwrap().revision, before + 1);
    let changed = store.changes_after(before, 100).await.unwrap();
    assert_eq!(changed.changes.len(), 1);
    assert_eq!(changed.changes[0].kind, "draft");
    assert_eq!(
        changed.changes[0].resource_id.as_deref(),
        Some(saved.id.as_str())
    );
    assert!(matches!(
        store.get_draft(other.clone(), saved.id.clone()).await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.delete_draft(other.clone(), saved.id.clone(), 2).await,
        Err(StoreError::NotFound)
    ));
    let second = store.save_draft(input(&account), 1002).await.unwrap();
    let page = store.query_drafts(account.clone(), 1, None).await.unwrap();
    assert_eq!(page.items[0].id, second.id);
    let token = page.next_page_token.unwrap();
    assert!(store
        .query_drafts(other.clone(), 1, Some(token.clone()))
        .await
        .is_err());
    assert!(store
        .query_drafts(account.clone(), 2, Some(token.clone()))
        .await
        .is_err());
    let next = store
        .query_drafts(account.clone(), 1, Some(token.clone()))
        .await
        .unwrap();
    assert_eq!(next.items[0].id, saved.id);
    assert!(next.next_page_token.is_none());
    store.close().await.unwrap();
    let bytes = std::fs::read(temp.path().join("store.db")).unwrap();
    assert!(!bytes
        .windows(b"private-draft-canary".len())
        .any(|w| w == b"private-draft-canary"));
    let store = Store::open(temp.path(), key).await.unwrap();
    let recovered = store
        .get_draft(account.clone(), saved.id.clone())
        .await
        .unwrap();
    assert_eq!(recovered.version, 2);
    assert_eq!(recovered.content.subject, "First edit");
    assert_eq!(
        recovered.content.text.as_deref(),
        Some("Private original body")
    );
    assert!(matches!(
        store
            .delete_draft(account.clone(), saved.id.clone(), 1)
            .await,
        Err(StoreError::VersionConflict)
    ));
    store
        .delete_draft(account.clone(), saved.id.clone(), 2)
        .await
        .unwrap();
    assert!(matches!(
        store.get_draft(account.clone(), saved.id).await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.query_drafts(account, 1, Some(token)).await,
        Err(StoreError::RefreshRequired)
    ));
    assert!(store
        .query_drafts(other, 100, None)
        .await
        .unwrap()
        .items
        .is_empty());
    store.close().await.unwrap();
}
