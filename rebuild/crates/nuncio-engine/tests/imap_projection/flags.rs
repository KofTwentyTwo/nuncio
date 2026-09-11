use super::*;
use nuncio_engine::{
    domain::{imap::ImapMessageState, mail_change::MailAction},
    store::{AttemptKind, AttemptOutcome, EnqueueMailChange, StoreError},
};

#[tokio::test]
async fn imap_flag_journal_is_account_scoped_fifo_and_applies_validated_receipts_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x71; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let a = AccountId::generate();
    let b = AccountId::generate();
    for account in [a, b] {
        store
            .add_account(AccountRecord {
                id: account.to_string(),
                provider: "imap".into(),
                address: "synthetic@example.test".into(),
            })
            .await
            .unwrap();
    }
    let run = begin(&store, a).await;
    let folder = store
        .stage_imap_mailbox(a.to_string(), run.clone(), mailbox("INBOX", 9001, 2))
        .await
        .unwrap();
    let placement = ImapPlacement::new(a, folder, 9001, 1).unwrap();
    store
        .stage_imap_mail(
            run.clone(),
            placement,
            ImapFlags::new(vec![]).unwrap(),
            mail(placement),
        )
        .await
        .unwrap();
    store
        .promote_mail(a.to_string(), run, None, 20)
        .await
        .unwrap();
    let local = store.query_mail(query(a)).await.unwrap().items[0]
        .id
        .clone();
    let input = EnqueueMailChange {
        account_id: a.to_string(),
        message_id: local.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        action: MailAction::Read { read: true },
    };
    let first = store.enqueue_mail_change(input.clone(), 30).await.unwrap();
    assert_eq!(
        store
            .enqueue_mail_change(input.clone(), 31)
            .await
            .unwrap()
            .id,
        first.id
    );
    let mut changed = input.clone();
    changed.action = MailAction::Read { read: false };
    assert!(matches!(
        store.enqueue_mail_change(changed.clone(), 31).await,
        Err(StoreError::VersionConflict)
    ));
    let mut foreign = input.clone();
    foreign.account_id = b.to_string();
    assert!(matches!(
        store.enqueue_mail_change(foreign, 31).await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store
            .imap_flag_payload(b.to_string(), first.id.clone())
            .await,
        Err(StoreError::NotFound)
    ));
    let intent = store
        .imap_flag_payload(a.to_string(), first.id.clone())
        .await
        .unwrap();
    assert_eq!(intent.placement, placement);
    assert_eq!(intent.mailbox.as_str(), "INBOX");
    assert!(intent.present);
    changed.request_id = uuid::Uuid::new_v4().to_string();
    let second = store.enqueue_mail_change(changed, 32).await.unwrap();
    assert!(matches!(
        store
            .begin_operation_attempt(a.to_string(), second.id.clone(), AttemptKind::Dispatch, 33)
            .await,
        Err(StoreError::VersionConflict)
    ));
    let attempt = store
        .begin_operation_attempt(a.to_string(), first.id.clone(), AttemptKind::Dispatch, 34)
        .await
        .unwrap();
    let unknown = store
        .finish_operation_attempt(
            a.to_string(),
            first.id.clone(),
            attempt.ordinal,
            AttemptOutcome::Uncertain {
                code: "lost_ack".into(),
            },
            35,
        )
        .await
        .unwrap();
    assert!(unknown.needs_reconciliation);
    let attempt = store
        .begin_operation_attempt(a.to_string(), first.id.clone(), AttemptKind::Reconcile, 36)
        .await
        .unwrap();
    for bad in [
        ImapMessageState {
            placement,
            flags: ImapFlags::new(vec![]).unwrap(),
        },
        ImapMessageState {
            placement: ImapPlacement::new(a, folder, 9002, 1).unwrap(),
            flags: ImapFlags::new(vec!["\\Seen".into()]).unwrap(),
        },
        ImapMessageState {
            placement: ImapPlacement::new(b, folder, 9001, 1).unwrap(),
            flags: ImapFlags::new(vec!["\\Seen".into()]).unwrap(),
        },
    ] {
        assert!(matches!(
            store
                .finish_operation_attempt(
                    a.to_string(),
                    first.id.clone(),
                    attempt.ordinal,
                    AttemptOutcome::AppliedImapFlags {
                        source: "positive_read".into(),
                        message: bad
                    },
                    37
                )
                .await,
            Err(StoreError::InvalidInput)
        ));
    }
    assert!(!store
        .get_mail(a.to_string(), local.clone())
        .await
        .unwrap()
        .provider_json
        .contains("Seen"));
    let receipt = ImapMessageState {
        placement,
        flags: ImapFlags::new(vec![
            "\\Seen".into(),
            "\\Answered".into(),
            "externalKeyword".into(),
        ])
        .unwrap(),
    };
    let done = store
        .finish_operation_attempt(
            a.to_string(),
            first.id.clone(),
            attempt.ordinal,
            AttemptOutcome::AppliedImapFlags {
                source: "positive_read".into(),
                message: receipt,
            },
            38,
        )
        .await
        .unwrap();
    assert_eq!(done.state, "applied");
    assert!(!done.needs_reconciliation);
    let stored: ImapMessageState = serde_json::from_str(
        &store
            .get_mail(a.to_string(), local.clone())
            .await
            .unwrap()
            .provider_json,
    )
    .unwrap();
    assert!(stored.flags.seen());
    assert!(stored.flags.values().contains(&"externalKeyword".into()));
    assert!(store
        .begin_operation_attempt(a.to_string(), second.id.clone(), AttemptKind::Dispatch, 39)
        .await
        .is_ok());
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let restored: ImapMessageState = serde_json::from_str(
        &store
            .get_mail(a.to_string(), local)
            .await
            .unwrap()
            .provider_json,
    )
    .unwrap();
    assert_eq!(restored.flags, stored.flags);
    assert_eq!(
        store
            .get_operation(a.to_string(), first.id)
            .await
            .unwrap()
            .state,
        "applied"
    );
    store.close().await.unwrap();
}
