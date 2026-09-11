#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        imap::{ImapFlags, ImapMailboxState, ImapPlacement, MailboxName},
        imap_account::ImapAccountConfig,
        mail::decode_mime,
        submission::{freeze, SubmissionTransport},
    },
    store::{
        AttemptKind, AttemptOutcome, ConnectedAccount, EnqueueSend, MailQuery, OperationReceipt,
        SaveDraft, SentCopyResult, SmtpStep, StagedMail, Store,
    },
};
use zeroize::Zeroizing;
#[tokio::test]
async fn smtp_delivery_and_sent_copy_are_separate_durable_effects() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x6a; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let config:ImapAccountConfig=serde_json::from_value(serde_json::json!({"address":"alpha@example.test","imap":{"host":"nas.example.test","port":993,"tls":"implicit","username":"alpha"},"smtp":{"host":"nas.example.test","port":587,"tls":"start_tls","username":"alpha"},"sent_policy":"client_append","sent_folder":"Sent"})).unwrap();
    store
        .prepare_credential("synthetic-smtp-ref".into())
        .await
        .unwrap();
    store
        .connect_imap(
            ConnectedAccount {
                id: account.clone(),
                subject: config.identity().unwrap(),
                address: config.address.clone(),
                credential_ref: "synthetic-smtp-ref".into(),
            },
            config,
            Default::default(),
        )
        .await
        .unwrap();
    let run = store
        .start_mail_run(account.clone(), "full".into(), 10)
        .await
        .unwrap();
    store
        .begin_sync_run(account.clone(), run.id.clone(), None)
        .await
        .unwrap();
    let state = ImapMailboxState {
        name: MailboxName::from_unicode("Sent").unwrap(),
        delimiter: Some('/'),
        attributes: vec![],
        uid_validity: Some(1234),
        uid_next: Some(1),
        highest_mod_seq: None,
    };
    let folder = store
        .stage_imap_mailbox(account.clone(), run.id.clone(), state.clone())
        .await
        .unwrap();
    store
        .promote_mail(account.clone(), run.id, None, 20)
        .await
        .unwrap();
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
    let frozen = freeze(
        "alpha@example.test",
        &content,
        None,
        &[],
        "smtp@nuncio.invalid",
        1000,
        SubmissionTransport::Smtp,
    )
    .unwrap();
    let raw = frozen.wire.clone();
    let op = store
        .enqueue_send(
            EnqueueSend {
                account_id: account.clone(),
                request_id: uuid::Uuid::new_v4().to_string(),
                draft_id: draft.id,
                expected_version: None,
                snapshot_version: 1,
                sender: "alpha@example.test".into(),
                frozen,
            },
            1001,
        )
        .await
        .unwrap();
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 1002)
        .await
        .unwrap();
    let copy = OperationReceipt {
        kind: "sent_copy".into(),
        source: "acknowledgement".into(),
        provider_id: Some("Sent/1234/56".into()),
        etag: None,
    };
    let before = store.status().await.unwrap().revision;
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                1,
                AttemptOutcome::Applied(copy),
                1003
            )
            .await
            .is_err(),
        "a copy cannot prove that SMTP accepted delivery"
    );
    assert_eq!(store.status().await.unwrap().revision, before);
    assert!(
        store
            .record_smtp_acceptance(account.clone(), op.id.clone(), 1, None, 1003)
            .await
            .is_err(),
        "SMTP acceptance requires a committed DATA dispatch marker"
    );
    let intent = store
        .smtp_intent(account.clone(), op.id.clone())
        .await
        .unwrap();
    assert_eq!(intent.mailbox_id, folder);
    assert_eq!(intent.uid_validity.get(), 1234);
    assert_eq!(intent.uid_next.get(), 1);
    assert_eq!(intent.endpoint.host, "nas.example.test");
    store
        .start_smtp_data(
            account.clone(),
            op.id.clone(),
            1,
            ImapMailboxState {
                uid_next: Some(5),
                ..state.clone()
            },
            1003,
        )
        .await
        .unwrap();
    let marked = store.status().await.unwrap().revision;
    assert_eq!(marked, before + 1);
    assert!(store
        .start_smtp_data(
            account.clone(),
            op.id.clone(),
            1,
            ImapMailboxState {
                uid_next: Some(5),
                ..state.clone()
            },
            1003
        )
        .await
        .is_err());
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                1,
                AttemptOutcome::Rejected {
                    code: "lost_ack".into(),
                    retry_at_ms: Some(1004)
                },
                1003
            )
            .await
            .is_err(),
        "no generic reset after DATA marker"
    );
    let accepted = store
        .record_smtp_acceptance(account.clone(), op.id.clone(), 1, None, 1003)
        .await
        .unwrap();
    assert_eq!(accepted.state, "running");
    assert_eq!(store.status().await.unwrap().revision, marked + 1);
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                1,
                AttemptOutcome::Rejected {
                    code: "sent_copy_denied".into(),
                    retry_at_ms: None
                },
                1004
            )
            .await
            .is_err(),
        "failed Sent copy cannot undo SMTP acceptance"
    );
    assert!(
        store
            .record_smtp_acceptance(account.clone(), op.id.clone(), 1, None, 1004)
            .await
            .is_err(),
        "there is only one SMTP dispatch acceptance"
    );
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    assert_eq!(store.recover_operations(1005).await.unwrap(), 1);
    let attempts = store
        .operation_attempts(account.clone(), op.id.clone())
        .await
        .unwrap();
    assert_eq!(attempts[0].receipts.len(), 1);
    assert_eq!(attempts[0].receipts[0].kind, "smtp_accepted");
    assert_eq!(attempts[0].receipts[0].observed_at_ms, 1003);
    assert!(
        attempts[0].receipts[0].provider_id.is_none(),
        "SMTP does not require the server to return a queue ID"
    );
    assert!(store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 1006)
        .await
        .is_err());
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 1006)
        .await
        .unwrap();
    assert_eq!(
        store
            .smtp_progress(account.clone(), op.id.clone())
            .await
            .unwrap()
            .step,
        SmtpStep::Accepted
    );
    assert!(store
        .start_smtp_data(
            account.clone(),
            op.id.clone(),
            2,
            ImapMailboxState {
                uid_next: Some(5),
                ..state.clone()
            },
            1006
        )
        .await
        .is_err());
    assert!(store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::RetryUnstartedSmtp { retry_at_ms: 1007 },
            1006
        )
        .await
        .is_err());
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                2,
                AttemptOutcome::Applied(OperationReceipt {
                    kind: "sent_copy".into(),
                    source: "positive_read".into(),
                    provider_id: Some("Sent/1234/56".into()),
                    etag: None
                }),
                1006
            )
            .await
            .is_err(),
        "an arbitrary receipt is not a Sent placement proof"
    );
    let placement = ImapPlacement::new(account.parse().unwrap(), folder, 1234, 56).unwrap();
    assert!(store
        .record_sent_append(account.clone(), op.id.clone(), 2, placement, 1006)
        .await
        .is_err());
    store
        .start_sent_append(account.clone(), op.id.clone(), 2, 1006)
        .await
        .unwrap();
    assert!(store
        .start_sent_append(account.clone(), op.id.clone(), 2, 1006)
        .await
        .is_err());
    store
        .record_sent_rejection(account.clone(), op.id.clone(), 2, 1006)
        .await
        .unwrap();
    assert_eq!(
        store
            .smtp_progress(account.clone(), op.id.clone())
            .await
            .unwrap()
            .step,
        SmtpStep::Accepted
    );
    assert!(store
        .record_sent_rejection(account.clone(), op.id.clone(), 2, 1006)
        .await
        .is_err());
    assert!(store
        .start_smtp_data(
            account.clone(),
            op.id.clone(),
            2,
            ImapMailboxState {
                uid_next: Some(5),
                ..state.clone()
            },
            1006
        )
        .await
        .is_err());
    store
        .start_sent_append(account.clone(), op.id.clone(), 2, 1006)
        .await
        .unwrap();
    let foreign = ImapPlacement::new(
        nuncio_engine::domain::identity::AccountId::generate(),
        folder,
        1234,
        56,
    )
    .unwrap();
    assert!(store
        .record_sent_append(account.clone(), op.id.clone(), 2, foreign, 1006)
        .await
        .is_err());
    store
        .record_sent_append(account.clone(), op.id.clone(), 2, placement, 1006)
        .await
        .unwrap();
    assert!(
        store
            .record_sent_rejection(account.clone(), op.id.clone(), 2, 1006)
            .await
            .is_err(),
        "a known APPENDUID cannot be reset for another copy"
    );
    let result = |bytes: Vec<u8>| SentCopyResult {
        placement,
        mailbox: ImapMailboxState {
            uid_next: Some(57),
            ..state.clone()
        },
        flags: ImapFlags::new(vec!["\\Seen".into()]).unwrap(),
        mail: StagedMail {
            provider_id: placement.provider_id().unwrap(),
            thread_id: None,
            history_id: None,
            internal_date_ms: Some(1003),
            provider_json: "{}".into(),
            subject: None,
            headers: vec![],
            labels: vec![],
            decoded: Some(decode_mime(&bytes, 64 * 1024 * 1024).unwrap()),
            raw: Some(bytes),
            availability: "available".into(),
        },
    };
    let revision = store.status().await.unwrap().revision;
    assert!(store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::AppliedSentCopy {
                result: Box::new(result(b"Subject: wrong\r\n\r\nwrong\r\n".to_vec()))
            },
            1007
        )
        .await
        .is_err());
    assert_eq!(
        store.status().await.unwrap().revision,
        revision,
        "wrong MIME must roll back publication"
    );
    let applied = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::AppliedSentCopy {
                result: Box::new(result(raw)),
            },
            1007,
        )
        .await
        .unwrap();
    let local = store
        .query_mail(MailQuery {
            account_id: account.clone(),
            collection_id: None,
            query: None,
            page_size: 100,
            page_token: None,
        })
        .await
        .unwrap();
    assert_eq!(local.items.len(), 1);
    assert_eq!(applied.state, "applied");
    let attempts = store
        .operation_attempts(account.clone(), op.id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].receipts[0].kind, "smtp_accepted");
    assert_eq!(attempts[1].receipts[0].kind, "imap_append_rejected");
    assert_eq!(attempts[1].receipts[1].kind, "imap_append");
    assert_eq!(attempts[1].receipts[2].kind, "sent_copy");
    let draft = store
        .save_draft(
            SaveDraft {
                account_id: account.clone(),
                id: None,
                expected_version: None,
                content: content.clone(),
            },
            2000,
        )
        .await
        .unwrap();
    let frozen = freeze(
        "alpha@example.test",
        &content,
        None,
        &[],
        "negative@nuncio.invalid",
        2000,
        SubmissionTransport::Smtp,
    )
    .unwrap();
    let op = store
        .enqueue_send(
            EnqueueSend {
                account_id: account.clone(),
                request_id: uuid::Uuid::new_v4().to_string(),
                draft_id: draft.id,
                expected_version: None,
                snapshot_version: 1,
                sender: "alpha@example.test".into(),
                frozen,
            },
            2001,
        )
        .await
        .unwrap();
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 2002)
        .await
        .unwrap();
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                1,
                AttemptOutcome::SmtpDataRejected {
                    reply_code: 450,
                    retry_at_ms: Some(2004)
                },
                2003
            )
            .await
            .is_err(),
        "final DATA rejection requires a started DATA attempt"
    );
    store
        .start_smtp_data(
            account.clone(),
            op.id.clone(),
            1,
            ImapMailboxState {
                uid_next: Some(57),
                ..state.clone()
            },
            2003,
        )
        .await
        .unwrap();
    for (reply_code, retry_at_ms) in [(250, None), (350, None), (550, Some(2004)), (650, None)] {
        assert!(store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                1,
                AttemptOutcome::SmtpDataRejected {
                    reply_code,
                    retry_at_ms
                },
                2003
            )
            .await
            .is_err());
    }
    let rejected = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            1,
            AttemptOutcome::SmtpDataRejected {
                reply_code: 450,
                retry_at_ms: Some(2004),
            },
            2003,
        )
        .await
        .unwrap();
    assert_eq!(rejected.state, "retry_wait");
    assert_eq!(
        store
            .smtp_progress(account.clone(), op.id.clone())
            .await
            .unwrap()
            .step,
        SmtpStep::Prepared
    );
    let history = store
        .operation_attempts(account.clone(), op.id.clone())
        .await
        .unwrap();
    assert_eq!(history[0].receipts.len(), 1);
    assert_eq!(history[0].receipts[0].kind, "smtp_rejected");
    assert_eq!(history[0].receipts[0].etag.as_deref(), Some("450"));
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(store.recover_operations(2004).await.unwrap(), 0);
    assert!(store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 2003)
        .await
        .is_err());
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 2004)
        .await
        .unwrap();
    store
        .start_smtp_data(
            account.clone(),
            op.id.clone(),
            2,
            ImapMailboxState {
                uid_next: Some(57),
                ..state.clone()
            },
            2005,
        )
        .await
        .unwrap();
    let rejected = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::SmtpDataRejected {
                reply_code: 550,
                retry_at_ms: None,
            },
            2006,
        )
        .await
        .unwrap();
    assert_eq!(rejected.state, "failed");
    assert!(store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 2007)
        .await
        .is_err());
    let history = store.operation_attempts(account, op.id).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].receipts[0].etag.as_deref(), Some("550"));
    assert!(history
        .iter()
        .flat_map(|a| &a.receipts)
        .all(|r| r.kind == "smtp_rejected"));
    store.close().await.unwrap();
}
