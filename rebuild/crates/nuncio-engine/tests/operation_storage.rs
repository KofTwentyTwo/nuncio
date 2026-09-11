#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        submission::{freeze, SubmissionTransport},
    },
    store::{AccountRecord, EnqueueSend, SaveDraft, Store, StoreError},
};
use zeroize::Zeroizing;
#[tokio::test]
async fn enqueued_send_is_atomic_idempotent_and_immutable_after_draft_edit_delete_or_restart() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x65; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    for id in [&account, &other] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "alpha@example.test".into(),
            })
            .await
            .unwrap();
    }
    let content = DraftContent {
        to: vec![Recipient {
            address: "recipient@example.test".into(),
            name: None,
        }],
        subject: "Frozen private intent".into(),
        text: Some("Immutable body".into()),
        ..Default::default()
    };
    let draft = store
        .save_draft(
            SaveDraft {
                account_id: account.clone(),
                id: None,
                expected_version: None,
                content: content.clone(),
            },
            1000,
        )
        .await
        .unwrap();
    let frozen = freeze(
        "alpha@example.test",
        &content,
        None,
        &[],
        "stable@nuncio.invalid",
        1000,
        SubmissionTransport::Gmail,
    )
    .unwrap();
    let request = EnqueueSend {
        account_id: account.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        draft_id: draft.id.clone(),
        expected_version: None,
        snapshot_version: 1,
        sender: "alpha@example.test".into(),
        frozen: frozen.clone(),
    };
    let before = store.status().await.unwrap().revision;
    let (first, second) = tokio::join!(
        store.enqueue_send(request.clone(), 1001),
        store.enqueue_send(request.clone(), 1001)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(first.state, "queued");
    assert_eq!(store.status().await.unwrap().revision, before + 1);
    assert_eq!(first.desired_state["draft_id"], draft.id);
    let changes = store.changes_after(before, 100).await.unwrap();
    assert_eq!(changes.changes.len(), 1);
    assert_eq!(changes.changes[0].kind, "operation");
    assert_eq!(
        changes.changes[0].resource_id.as_deref(),
        Some(first.id.as_str())
    );
    let mut changed = request.clone();
    changed.expected_version = Some(1);
    assert!(matches!(
        store.enqueue_send(changed, 1002).await,
        Err(StoreError::VersionConflict)
    ));
    assert!(matches!(
        store.get_operation(other.clone(), first.id.clone()).await,
        Err(StoreError::NotFound)
    ));
    let changed = DraftContent {
        subject: "Later mutable draft".into(),
        ..content
    };
    store
        .save_draft(
            SaveDraft {
                account_id: account.clone(),
                id: Some(draft.id.clone()),
                expected_version: Some(1),
                content: changed,
            },
            1003,
        )
        .await
        .unwrap();
    assert_eq!(
        store.enqueue_send(request.clone(), 1004).await.unwrap().id,
        first.id
    );
    let mut stale = request.clone();
    stale.request_id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        store.enqueue_send(stale, 1004).await,
        Err(StoreError::VersionConflict)
    ));
    store
        .delete_draft(account.clone(), draft.id, 2)
        .await
        .unwrap();
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let loaded = store
        .get_operation(account.clone(), first.id.clone())
        .await
        .unwrap();
    assert_eq!(loaded.state, "queued");
    let payload = store
        .send_payload(account.clone(), first.id.clone())
        .await
        .unwrap();
    assert_eq!(payload.message_id, "stable@nuncio.invalid");
    assert_eq!(payload.wire.byte_length, frozen.wire.len() as u64);
    assert_eq!(
        store
            .blob_chunk(account.clone(), payload.wire.id, 0)
            .await
            .unwrap(),
        frozen.wire
    );
    assert_eq!(
        store.enqueue_send(request, 1005).await.unwrap().id,
        first.id
    );
    assert_eq!(
        store
            .cancel_operation(account.clone(), first.id.clone(), 1006)
            .await
            .unwrap()
            .state,
        "cancelled"
    );
    let revision = store.status().await.unwrap().revision;
    assert_eq!(
        store
            .cancel_operation(account, first.id, 1007)
            .await
            .unwrap()
            .state,
        "cancelled"
    );
    assert_eq!(store.status().await.unwrap().revision, revision);
    store.close().await.unwrap();
    let ciphertext = std::fs::read(temp.path().join("store.db")).unwrap();
    assert!(!ciphertext
        .windows(b"Immutable body".len())
        .any(|w| w == b"Immutable body"));
}
