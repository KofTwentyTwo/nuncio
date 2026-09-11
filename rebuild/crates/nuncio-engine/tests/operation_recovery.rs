#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        submission::{freeze, SubmissionTransport},
    },
    store::{
        AccountRecord, AttemptKind, AttemptOutcome, EnqueueSend, OperationReceipt, SaveDraft,
        Store, StoreError,
    },
};
use zeroize::Zeroizing;
#[tokio::test]
async fn running_send_recovers_for_reconciliation_and_cannot_be_blindly_redispatched_or_cancelled()
{
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x61; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    store
        .add_account(AccountRecord {
            id: account.clone(),
            provider: "google".into(),
            address: "alpha@example.test".into(),
        })
        .await
        .unwrap();
    let content = DraftContent {
        to: vec![Recipient {
            address: "recipient@example.test".into(),
            name: None,
        }],
        text: Some("Recovery body".into()),
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
        "recovery@nuncio.invalid",
        1000,
        SubmissionTransport::Gmail,
    )
    .unwrap();
    let operation = store
        .enqueue_send(
            EnqueueSend {
                account_id: account.clone(),
                request_id: uuid::Uuid::new_v4().to_string(),
                draft_id: draft.id,
                expected_version: Some(1),
                snapshot_version: 1,
                sender: "alpha@example.test".into(),
                frozen,
            },
            1001,
        )
        .await
        .unwrap();
    let first = store
        .begin_operation_attempt(
            account.clone(),
            operation.id.clone(),
            AttemptKind::Dispatch,
            1002,
        )
        .await
        .unwrap();
    assert_eq!(first.ordinal, 1);
    assert_eq!(
        store
            .get_operation(account.clone(), operation.id.clone())
            .await
            .unwrap()
            .state,
        "running"
    );
    assert!(matches!(
        store
            .cancel_operation(account.clone(), operation.id.clone(), 1003)
            .await,
        Err(StoreError::VersionConflict)
    ));
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let before = store.status().await.unwrap().revision;
    assert_eq!(store.recover_operations(1004).await.unwrap(), 1);
    assert_eq!(store.recover_operations(1004).await.unwrap(), 0);
    let recovered = store
        .get_operation(account.clone(), operation.id.clone())
        .await
        .unwrap();
    assert_eq!(recovered.state, "uncertain");
    assert!(recovered.needs_reconciliation);
    assert_eq!(store.status().await.unwrap().revision, before + 1);
    assert!(matches!(
        store
            .begin_operation_attempt(
                account.clone(),
                operation.id.clone(),
                AttemptKind::Dispatch,
                1005
            )
            .await,
        Err(StoreError::VersionConflict)
    ));
    assert!(matches!(
        store
            .cancel_operation(account.clone(), operation.id.clone(), 1005)
            .await,
        Err(StoreError::VersionConflict)
    ));
    let reconcile = store
        .begin_operation_attempt(
            account.clone(),
            operation.id.clone(),
            AttemptKind::Reconcile,
            1005,
        )
        .await
        .unwrap();
    assert_eq!(reconcile.ordinal, 2);
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                operation.id.clone(),
                2,
                AttemptOutcome::Rejected {
                    code: "not_found".into(),
                    retry_at_ms: None
                },
                1006
            )
            .await
            .is_err(),
        "a missing reconciliation read cannot establish non-delivery"
    );
    for receipt in [
        OperationReceipt {
            kind: "google_send".into(),
            source: "positive_read".into(),
            provider_id: None,
            etag: None,
        },
        OperationReceipt {
            kind: "google_send".into(),
            source: "manual_confirmation".into(),
            provider_id: Some("claimed".into()),
            etag: None,
        },
    ] {
        let before = store.status().await.unwrap().revision;
        assert!(store.finish_operation_attempt(account.clone(), operation.id.clone(), 2, AttemptOutcome::Applied(receipt), 1006).await.is_err(), "a provider attempt requires positive provider evidence; manual decisions use the audited resolution path");
        assert_eq!(store.status().await.unwrap().revision, before);
    }
    let applied = store
        .finish_operation_attempt(
            account.clone(),
            operation.id.clone(),
            2,
            AttemptOutcome::Applied(OperationReceipt {
                kind: "google_send".into(),
                source: "positive_read".into(),
                provider_id: Some("remote-accepted".into()),
                etag: None,
            }),
            1006,
        )
        .await
        .unwrap();
    assert_eq!(applied.state, "applied");
    assert!(!applied.needs_reconciliation);
    let attempts = store
        .operation_attempts(account.clone(), operation.id.clone())
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome.as_deref(), Some("uncertain"));
    assert_eq!(attempts[1].receipts.len(), 1);
    assert_eq!(
        attempts[1].receipts[0].provider_id.as_deref(),
        Some("remote-accepted")
    );
    assert!(store
        .begin_operation_attempt(
            account.clone(),
            operation.id.clone(),
            AttemptKind::Dispatch,
            1007
        )
        .await
        .is_err());
    assert!(store
        .cancel_operation(account, operation.id, 1007)
        .await
        .is_err());
    store.close().await.unwrap();
}
