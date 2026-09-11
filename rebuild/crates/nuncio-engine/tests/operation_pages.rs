#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        submission::{freeze, SubmissionTransport},
    },
    store::{
        AccountRecord, AttemptKind, AttemptOutcome, EnqueueSend, SaveDraft, Store, StoreError,
    },
};
use zeroize::Zeroizing;
#[tokio::test]
async fn operation_and_attempt_pages_are_complete_scoped_and_revision_bound() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x69; 32]))
        .await
        .unwrap();
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
    let mut ids = Vec::new();
    for i in 0..5 {
        let frozen = freeze(
            "alpha@example.test",
            &content,
            None,
            &[],
            &format!("request-{i}@nuncio.invalid"),
            1000,
            SubmissionTransport::Gmail,
        )
        .unwrap();
        ids.push(
            store
                .enqueue_send(
                    EnqueueSend {
                        account_id: account.clone(),
                        request_id: uuid::Uuid::new_v4().to_string(),
                        draft_id: draft.id.clone(),
                        expected_version: None,
                        snapshot_version: 1,
                        sender: "alpha@example.test".into(),
                        frozen,
                    },
                    1001 + i,
                )
                .await
                .unwrap()
                .id,
        );
    }
    let page = store
        .query_operations(account.clone(), 2, None)
        .await
        .unwrap();
    assert_eq!(
        page.items.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
        [ids[4].as_str(), ids[3].as_str()]
    );
    let token = page.next_page_token.unwrap();
    assert!(store
        .query_operations(other, 2, Some(token.clone()))
        .await
        .is_err());
    assert!(store
        .query_operations(account.clone(), 1, Some(token.clone()))
        .await
        .is_err());
    let second = store
        .query_operations(account.clone(), 2, Some(token.clone()))
        .await
        .unwrap();
    assert_eq!(second.items[0].id, ids[2]);
    let third = store
        .query_operations(account.clone(), 2, second.next_page_token)
        .await
        .unwrap();
    assert_eq!(third.items.len(), 1);
    assert_eq!(third.items[0].id, ids[0]);
    assert!(third.next_page_token.is_none());
    let id = ids[4].clone();
    store
        .begin_operation_attempt(account.clone(), id.clone(), AttemptKind::Dispatch, 2000)
        .await
        .unwrap();
    store
        .finish_operation_attempt(
            account.clone(),
            id.clone(),
            1,
            AttemptOutcome::Rejected {
                code: "connection_refused".into(),
                retry_at_ms: Some(2002),
            },
            2001,
        )
        .await
        .unwrap();
    assert!(store
        .begin_operation_attempt(account.clone(), id.clone(), AttemptKind::Dispatch, 2001)
        .await
        .is_err());
    store
        .begin_operation_attempt(account.clone(), id.clone(), AttemptKind::Dispatch, 2002)
        .await
        .unwrap();
    store
        .finish_operation_attempt(
            account.clone(),
            id.clone(),
            2,
            AttemptOutcome::Uncertain {
                code: "acknowledgement_lost".into(),
            },
            2003,
        )
        .await
        .unwrap();
    store
        .begin_operation_attempt(account.clone(), id.clone(), AttemptKind::Reconcile, 2004)
        .await
        .unwrap();
    let unresolved = store
        .finish_operation_attempt(
            account.clone(),
            id.clone(),
            3,
            AttemptOutcome::Uncertain {
                code: "no_positive_evidence".into(),
            },
            2005,
        )
        .await
        .unwrap();
    assert_eq!(unresolved.state, "uncertain");
    assert!(!unresolved.needs_reconciliation);
    assert!(matches!(
        store
            .query_operations(account.clone(), 2, Some(token))
            .await,
        Err(StoreError::RefreshRequired)
    ));
    let first = store
        .query_operation_attempts(account.clone(), id.clone(), 2, None)
        .await
        .unwrap();
    assert_eq!(
        first.items.iter().map(|a| a.ordinal).collect::<Vec<_>>(),
        [3, 2]
    );
    let token = first.next_page_token.unwrap();
    assert!(store
        .query_operation_attempts(account.clone(), ids[0].clone(), 2, Some(token.clone()))
        .await
        .is_err());
    let second = store
        .query_operation_attempts(account.clone(), id, 2, Some(token))
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].outcome.as_deref(), Some("rejected"));
    assert!(second.next_page_token.is_none());
    let id = ids[0].clone();
    store
        .begin_operation_attempt(account.clone(), id.clone(), AttemptKind::Dispatch, 3000)
        .await
        .unwrap();
    let immediate = store
        .finish_operation_attempt(
            account.clone(),
            id.clone(),
            1,
            AttemptOutcome::Rejected {
                code: "rate_limited".into(),
                retry_at_ms: Some(2999),
            },
            3001,
        )
        .await
        .unwrap();
    assert_eq!(immediate.state, "retry_wait");
    assert_eq!(
        immediate.next_attempt_at_ms,
        Some(3001),
        "an elapsed provider deadline means immediately eligible, not an invalid transition"
    );
    store
        .begin_operation_attempt(account.clone(), id.clone(), AttemptKind::Dispatch, 3001)
        .await
        .unwrap();
    let before = store.status().await.unwrap().revision;
    for invalid in [
        AttemptOutcome::Repeatable {
            code: "unknown_send".into(),
            retry_at_ms: 3003,
        },
        AttemptOutcome::Conflict {
            code: "send_conflict".into(),
        },
    ] {
        assert!(store
            .finish_operation_attempt(account.clone(), id.clone(), 2, invalid, 3002)
            .await
            .is_err());
        assert_eq!(store.status().await.unwrap().revision, before);
    }
    let rejected = store
        .finish_operation_attempt(
            account.clone(),
            id.clone(),
            2,
            AttemptOutcome::Rejected {
                code: "invalid_recipient".into(),
                retry_at_ms: None,
            },
            3002,
        )
        .await
        .unwrap();
    assert_eq!(rejected.state, "failed");
    assert!(!rejected.needs_reconciliation);
    assert!(store
        .begin_operation_attempt(account.clone(), id.clone(), AttemptKind::Dispatch, 3003)
        .await
        .is_err());
    assert!(store.cancel_operation(account, id, 3003).await.is_err());
    store.close().await.unwrap();
}
