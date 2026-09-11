#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::mail_change::MailAction,
    store::{
        AttemptKind, AttemptOutcome, ConnectedAccount, EnqueueMailChange, MailChangeReceipt,
        MailQuery, OperationReceipt, StagedMail, Store, StoreError,
    },
};
use zeroize::Zeroizing;

async fn project(store: &Store, account: &str, present: bool) -> Option<String> {
    let run = store
        .start_mail_run(account.into(), "full".into(), 1)
        .await
        .unwrap();
    store
        .begin_sync_run(account.into(), run.id.clone(), Some("10".into()))
        .await
        .unwrap();
    if present {
        for (label, kind) in [
            ("INBOX", "system"),
            ("UNREAD", "system"),
            ("Label_keep", "user"),
        ] {
            store
                .stage_mail_collection(
                    account.into(),
                    run.id.clone(),
                    label.into(),
                    Some(label.into()),
                    kind.into(),
                )
                .await
                .unwrap();
        }
        store
            .stage_mail(
                account.into(),
                run.id.clone(),
                StagedMail {
                    provider_id: "remote-one".into(),
                    thread_id: None,
                    history_id: Some("10".into()),
                    internal_date_ms: Some(1),
                    provider_json: "{}".into(),
                    subject: None,
                    headers: vec![],
                    labels: vec!["INBOX".into(), "UNREAD".into(), "Label_keep".into()],
                    raw: None,
                    decoded: None,
                    availability: "missing".into(),
                },
            )
            .await
            .unwrap();
    }
    store
        .promote_mail(account.into(), run.id, Some("10".into()), 2)
        .await
        .unwrap();
    store
        .query_mail(MailQuery {
            account_id: account.into(),
            collection_id: None,
            query: None,
            page_size: 100,
            page_token: None,
        })
        .await
        .unwrap()
        .items
        .first()
        .map(|m| m.id.clone())
}

#[tokio::test]
async fn desired_mail_changes_are_atomic_scoped_ordered_and_survive_projection_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x61; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    for id in [&account, &other] {
        let reference = format!("synthetic/{id}");
        store.prepare_credential(reference.clone()).await.unwrap();
        store
            .connect_google(ConnectedAccount {
                id: id.clone(),
                subject: id.clone(),
                address: "alpha@example.test".into(),
                credential_ref: reference,
            })
            .await
            .unwrap();
    }
    let message = project(&store, &account, true).await.unwrap();
    project(&store, &other, true).await;
    let request = EnqueueMailChange {
        account_id: account.clone(),
        message_id: message.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        action: MailAction::Read { read: true },
    };
    let before = store.status().await.unwrap().revision;
    let op = store
        .enqueue_mail_change(request.clone(), 100)
        .await
        .unwrap();
    assert_eq!(op.state, "queued");
    assert_eq!(op.kind, "mail_change");
    assert_eq!(op.desired_state["provider_message_id"], "remote-one");
    assert_eq!(
        op.desired_state["remove_label_ids"],
        serde_json::json!(["UNREAD"])
    );
    assert_eq!(store.status().await.unwrap().revision, before + 1);
    let again = store
        .enqueue_mail_change(request.clone(), 101)
        .await
        .unwrap();
    assert_eq!(op.id, again.id);
    assert_eq!(store.status().await.unwrap().revision, before + 1);
    let mut changed = request.clone();
    changed.action = MailAction::Read { read: false };
    assert!(matches!(
        store.enqueue_mail_change(changed.clone(), 102).await,
        Err(StoreError::VersionConflict)
    ));
    changed.account_id = other.clone();
    assert!(matches!(
        store.enqueue_mail_change(changed, 102).await,
        Err(StoreError::NotFound)
    ));
    let observed = store
        .get_mail(account.clone(), message.clone())
        .await
        .unwrap();
    assert!(
        observed
            .message
            .collections
            .iter()
            .any(|c| c.provider_id == "UNREAD"),
        "unacknowledged intent must not masquerade as provider observation"
    );
    let later = store
        .enqueue_mail_change(
            EnqueueMailChange {
                request_id: uuid::Uuid::new_v4().to_string(),
                action: MailAction::Read { read: false },
                ..request.clone()
            },
            100,
        )
        .await
        .unwrap();
    assert_eq!(
        store.ready_operations(100).await.unwrap()[0].id,
        op.id,
        "insertion order breaks timestamp ties"
    );
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 103)
        .await
        .unwrap();
    store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            1,
            AttemptOutcome::Repeatable {
                code: "response_lost".into(),
                retry_at_ms: 1000,
            },
            104,
        )
        .await
        .unwrap();
    assert!(
        store.ready_operations(999).await.unwrap().is_empty(),
        "a later inverse change cannot overtake a delayed earlier change"
    );
    project(&store, &account, false).await;
    assert_eq!(
        store
            .enqueue_mail_change(request.clone(), 105)
            .await
            .unwrap()
            .id,
        op.id
    );
    assert_eq!(
        store
            .mail_change_payload(account.clone(), op.id.clone())
            .await
            .unwrap()
            .provider_message_id,
        "remote-one"
    );
    let replacement = project(&store, &account, true).await.unwrap();
    assert_ne!(replacement, message);
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 1000)
        .await
        .unwrap();
    let before = store.status().await.unwrap().revision;
    let bad = MailChangeReceipt {
        provider_message_id: "different".into(),
        history_id: Some("11".into()),
        label_ids: vec!["INBOX".into(), "Label_keep".into()],
    };
    assert!(store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::AppliedMail {
                source: "acknowledgement".into(),
                message: bad
            },
            1001
        )
        .await
        .is_err());
    assert_eq!(store.status().await.unwrap().revision, before);
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                2,
                AttemptOutcome::Applied(OperationReceipt {
                    kind: "mail_change".into(),
                    source: "acknowledgement".into(),
                    provider_id: Some("remote-one".into()),
                    etag: None
                }),
                1001
            )
            .await
            .is_err(),
        "mail changes require authoritative memberships in the atomic receipt transaction"
    );
    let receipt = MailChangeReceipt {
        provider_message_id: "remote-one".into(),
        history_id: Some("9007199254741019".into()),
        label_ids: vec!["INBOX".into(), "Label_keep".into(), "Label_external".into()],
    };
    let applied = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::AppliedMail {
                source: "acknowledgement".into(),
                message: receipt,
            },
            1001,
        )
        .await
        .unwrap();
    assert_eq!(applied.state, "applied");
    assert_eq!(store.status().await.unwrap().revision, before + 2);
    let local = store.get_mail(account.clone(), replacement).await.unwrap();
    let mut labels: Vec<_> = local
        .message
        .collections
        .iter()
        .map(|c| c.provider_id.as_str())
        .collect();
    labels.sort();
    assert_eq!(labels, ["INBOX", "Label_external", "Label_keep"]);
    assert_eq!(
        local.message.history_id.as_deref(),
        Some("9007199254741019")
    );
    assert_eq!(store.ready_operations(1002).await.unwrap()[0].id, later.id);
    store
        .cancel_operation(account.clone(), later.id, 1002)
        .await
        .unwrap();
    store.close().await.unwrap();
    let reopened = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(
        reopened
            .enqueue_mail_change(request, 1003)
            .await
            .unwrap()
            .state,
        "applied"
    );
    assert_eq!(
        reopened.operation_attempts(account, op.id).await.unwrap()[1].receipts[0]
            .provider_id
            .as_deref(),
        Some("remote-one")
    );
    reopened.close().await.unwrap();
}
