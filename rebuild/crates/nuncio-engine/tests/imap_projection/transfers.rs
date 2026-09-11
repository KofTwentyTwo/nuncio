#![allow(clippy::panic)]
use super::*;
use nuncio_engine::{
    domain::{imap_account::ImapAccountConfig, mail_change::MailAction},
    store::{
        AttemptKind, AttemptOutcome, ConnectedAccount, EnqueueMailChange, ImapChangePayload,
        ImapCopyProof, ImapTransferMode, ImapTransferResult, ImapTransferStep, StoreError,
    },
};

#[tokio::test]
async fn transfer_intent_and_copy_progress_survive_reopen_and_cannot_repeat_a_started_copy() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x73; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = AccountId::generate();
    let config:ImapAccountConfig=serde_json::from_value(serde_json::json!({"address":"alpha@example.test","imap":{"host":"nas.example.test","port":993,"tls":"implicit","username":"alpha"},"smtp":{"host":"nas.example.test","port":587,"tls":"start_tls","username":"alpha"},"sent_policy":"client_append","sent_folder":"Sent","archive_folder":"Archive","trash_folder":"Trash"})).unwrap();
    store
        .prepare_credential("synthetic-transfer-reference".into())
        .await
        .unwrap();
    store
        .connect_imap(
            ConnectedAccount {
                id: account.to_string(),
                subject: config.identity().unwrap(),
                address: config.address.clone(),
                credential_ref: "synthetic-transfer-reference".into(),
            },
            config,
            Default::default(),
        )
        .await
        .unwrap();
    let run = begin(&store, account).await;
    let inbox = store
        .stage_imap_mailbox(account.to_string(), run.clone(), mailbox("INBOX", 9001, 2))
        .await
        .unwrap();
    let archive = store
        .stage_imap_mailbox(
            account.to_string(),
            run.clone(),
            mailbox("Archive", 9010, 1),
        )
        .await
        .unwrap();
    let source = ImapPlacement::new(account, inbox, 9001, 1).unwrap();
    store
        .stage_imap_mail(
            run.clone(),
            source,
            ImapFlags::new(vec![]).unwrap(),
            mail(source),
        )
        .await
        .unwrap();
    store
        .promote_mail(account.to_string(), run, None, 20)
        .await
        .unwrap();
    let id = store.query_mail(query(account)).await.unwrap().items[0]
        .id
        .clone();
    let input = EnqueueMailChange {
        account_id: account.to_string(),
        message_id: id,
        request_id: uuid::Uuid::new_v4().to_string(),
        action: MailAction::Archive {},
    };
    let op = store.enqueue_mail_change(input.clone(), 21).await.unwrap();
    assert_eq!(
        store.enqueue_mail_change(input, 22).await.unwrap().id,
        op.id
    );
    let ImapChangePayload::Transfer(payload) = store
        .imap_change_payload(account.to_string(), op.id.clone())
        .await
        .unwrap()
    else {
        panic!("archive did not capture a transfer")
    };
    assert_eq!(payload.source, source);
    assert_eq!(payload.destination, archive);
    assert_eq!(payload.destination_uid_validity.get(), 9010);
    assert_eq!(payload.mode, ImapTransferMode::Move);
    let attempt = store
        .begin_operation_attempt(
            account.to_string(),
            op.id.clone(),
            AttemptKind::Dispatch,
            23,
        )
        .await
        .unwrap();
    assert!(store
        .record_imap_copy(
            account.to_string(),
            op.id.clone(),
            attempt.ordinal,
            ImapCopyProof {
                source,
                destination: ImapPlacement::new(account, archive, 9010, 1).unwrap()
            },
            24
        )
        .await
        .is_err());
    store
        .start_imap_transfer(
            account.to_string(),
            op.id.clone(),
            attempt.ordinal,
            ImapTransferMode::Copy,
            24,
        )
        .await
        .unwrap();
    let proof = ImapCopyProof {
        source,
        destination: ImapPlacement::new(account, archive, 9010, 1).unwrap(),
    };
    let wrong = ImapCopyProof {
        source,
        destination: ImapPlacement::new(account, archive, 9011, 1).unwrap(),
    };
    assert!(matches!(
        store
            .record_imap_copy(
                account.to_string(),
                op.id.clone(),
                attempt.ordinal,
                wrong,
                25
            )
            .await,
        Err(StoreError::InvalidInput)
    ));
    store
        .record_imap_copy(
            account.to_string(),
            op.id.clone(),
            attempt.ordinal,
            proof.clone(),
            26,
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .finish_operation_attempt(
                account.to_string(),
                op.id.clone(),
                attempt.ordinal,
                AttemptOutcome::Rejected {
                    code: "post_copy_failure".into(),
                    retry_at_ms: Some(27)
                },
                27
            )
            .await,
        Err(StoreError::InvalidInput)
    ));
    store
        .finish_operation_attempt(
            account.to_string(),
            op.id.clone(),
            attempt.ordinal,
            AttemptOutcome::Uncertain {
                code: "crashed_after_copy".into(),
            },
            27,
        )
        .await
        .unwrap();
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let progress = store
        .imap_transfer_progress(account.to_string(), op.id.clone())
        .await
        .unwrap();
    assert_eq!(progress.phase, ImapTransferStep::Copied);
    assert_eq!(progress.copy.unwrap().destination, proof.destination);
    let next = store
        .begin_operation_attempt(
            account.to_string(),
            op.id.clone(),
            AttemptKind::Reconcile,
            28,
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .start_imap_transfer(
                account.to_string(),
                op.id.clone(),
                next.ordinal,
                ImapTransferMode::Copy,
                29
            )
            .await,
        Err(StoreError::VersionConflict)
    ));
    assert!(matches!(
        store
            .finish_operation_attempt(
                account.to_string(),
                op.id.clone(),
                next.ordinal,
                AttemptOutcome::Repeatable {
                    code: "must_not_recopy".into(),
                    retry_at_ms: 30
                },
                29
            )
            .await,
        Err(StoreError::InvalidInput)
    ));
    store
        .advance_imap_transfer(
            account.to_string(),
            op.id.clone(),
            next.ordinal,
            ImapTransferStep::DeletingSource,
            30,
        )
        .await
        .unwrap();
    store
        .advance_imap_transfer(
            account.to_string(),
            op.id.clone(),
            next.ordinal,
            ImapTransferStep::ExpungingSource,
            31,
        )
        .await
        .unwrap();
    store
        .advance_imap_transfer(
            account.to_string(),
            op.id.clone(),
            next.ordinal,
            ImapTransferStep::SourceRemoved,
            32,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .imap_transfer_progress(account.to_string(), op.id.clone())
            .await
            .unwrap()
            .phase,
        ImapTransferStep::SourceRemoved
    );
    let before = store.query_mail(query(account)).await.unwrap();
    assert_eq!(before.items.len(), 1);
    let source_id = before.items[0].id.clone();
    let result = ImapTransferResult {
        proof: proof.clone(),
        mailbox: mailbox("Archive", 9010, 2),
        flags: ImapFlags::new(vec!["\\Seen".into()]).unwrap(),
        mail: mail(proof.destination),
    };
    let done = store
        .finish_operation_attempt(
            account.to_string(),
            op.id.clone(),
            next.ordinal,
            AttemptOutcome::AppliedImapTransfer {
                source: "positive_read".into(),
                result: Box::new(result),
            },
            33,
        )
        .await
        .unwrap();
    assert_eq!(done.state, "applied");
    let after = store.query_mail(query(account)).await.unwrap();
    assert_eq!(after.items.len(), 1);
    assert_ne!(after.items[0].id, source_id);
    assert_eq!(after.items[0].collections.len(), 1);
    assert_eq!(
        after.items[0].collections[0].name.as_deref(),
        Some("Archive")
    );
    assert!(matches!(
        store.get_mail(account.to_string(), source_id).await,
        Err(StoreError::NotFound)
    ));
    let copied = store
        .get_mail(account.to_string(), after.items[0].id.clone())
        .await
        .unwrap();
    let state: nuncio_engine::domain::imap::ImapMessageState =
        serde_json::from_str(&copied.provider_json).unwrap();
    assert_eq!(state.placement, proof.destination);
    assert!(state.flags.seen());
    assert_eq!(after.coverage.cursor, before.coverage.cursor);
    store.close().await.unwrap();
}
