#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        imap::{ImapFlags, ImapMailboxState, ImapPlacement, MailboxName},
        imap_account::ImapAccountConfig,
        mail::decode_mime,
        submission::{freeze, SentFingerprint, SubmissionTransport},
    },
    store::{
        AttemptKind, AttemptOutcome, ConnectedAccount, EnqueueSend, MailQuery, SaveDraft,
        SentCopyResult, ServerSentEvidence, SmtpStep, StagedMail, Store,
    },
};
use zeroize::Zeroizing;
#[tokio::test]
async fn server_sent_requires_accepted_smtp_fresh_floor_and_faithful_content_across_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x6a; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let config:ImapAccountConfig=serde_json::from_value(serde_json::json!({"address":"alpha@example.test","imap":{"host":"nas.example.test","port":993,"tls":"implicit","username":"alpha"},"smtp":{"host":"nas.example.test","port":587,"tls":"start_tls","username":"alpha"},"sent_policy":"server","sent_folder":"Sent"})).unwrap();
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
            address: "beta@example.test".into(),
            name: None,
        }],
        bcc: vec![Recipient {
            address: "blind@example.test".into(),
            name: None,
        }],
        subject: "Server Sent".into(),
        text: Some("Frozen body".into()),
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
        "server@nuncio.invalid",
        1000,
        SubmissionTransport::Smtp,
    )
    .unwrap();
    let mut raw = b"Received: from local by server; Thu, 01 Jan 1970 00:00:02 +0000\r\n".to_vec();
    raw.extend_from_slice(&frozen.wire);
    let fingerprint = SentFingerprint::from_mime(&raw).unwrap();
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
    let intent = store
        .smtp_intent(account.clone(), op.id.clone())
        .await
        .unwrap();
    assert!(intent
        .sent_fingerprint
        .as_ref()
        .unwrap()
        .matches(&fingerprint));
    assert_eq!(intent.uid_next.get(), 1);
    let observed = ImapMailboxState {
        uid_next: Some(10),
        ..state.clone()
    };
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 1002)
        .await
        .unwrap();
    store
        .start_smtp_data(account.clone(), op.id.clone(), 1, observed, 1002)
        .await
        .unwrap();
    let target = ImapPlacement::new(account.parse().unwrap(), folder, 1234, 10).unwrap();
    let evidence = |placement| ServerSentEvidence {
        placement,
        fingerprint: fingerprint.clone(),
    };
    assert!(
        store
            .record_server_sent(account.clone(), op.id.clone(), 1, evidence(target), 1003)
            .await
            .is_err(),
        "a Sent copy cannot prove SMTP acceptance"
    );
    store
        .record_smtp_acceptance(account.clone(), op.id.clone(), 1, None, 1003)
        .await
        .unwrap();
    assert!(
        store
            .start_sent_append(account.clone(), op.id.clone(), 1, 1003)
            .await
            .is_err(),
        "server policy cannot append a client copy"
    );
    let older = ImapPlacement::new(account.parse().unwrap(), folder, 1234, 9).unwrap();
    assert!(
        store
            .record_server_sent(account.clone(), op.id.clone(), 1, evidence(older), 1003)
            .await
            .is_err(),
        "matching MIME older than the pre-DATA floor is not this server copy"
    );
    let wrong = SentFingerprint::from_mime(
        String::from_utf8(raw.clone())
            .unwrap()
            .replace("Frozen body", "Other body")
            .as_bytes(),
    )
    .unwrap();
    assert!(store
        .record_server_sent(
            account.clone(),
            op.id.clone(),
            1,
            ServerSentEvidence {
                placement: target,
                fingerprint: wrong
            },
            1003
        )
        .await
        .is_err());
    store
        .record_server_sent(account.clone(), op.id.clone(), 1, evidence(target), 1004)
        .await
        .unwrap();
    let progress = store
        .smtp_progress(account.clone(), op.id.clone())
        .await
        .unwrap();
    assert_eq!(progress.step, SmtpStep::Copied);
    assert_eq!(progress.sent_floor.unwrap().get(), 10);
    assert_eq!(progress.placement, Some(target));
    assert!(store
        .record_sent_rejection(account.clone(), op.id.clone(), 1, 1004)
        .await
        .is_err());
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(store.recover_operations(1005).await.unwrap(), 1);
    assert_eq!(
        store
            .smtp_progress(account.clone(), op.id.clone())
            .await
            .unwrap(),
        progress
    );
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 1006)
        .await
        .unwrap();
    let result = |raw: Vec<u8>| SentCopyResult {
        placement: target,
        mailbox: ImapMailboxState {
            uid_next: Some(11),
            ..state.clone()
        },
        flags: ImapFlags::new(vec!["\\Seen".into()]).unwrap(),
        mail: StagedMail {
            provider_id: target.provider_id().unwrap(),
            thread_id: None,
            history_id: None,
            internal_date_ms: Some(1003),
            provider_json: "{}".into(),
            subject: None,
            headers: vec![],
            labels: vec![],
            decoded: Some(decode_mime(&raw, 64 * 1024 * 1024).unwrap()),
            raw: Some(raw),
            availability: "available".into(),
        },
    };
    let before = store.status().await.unwrap().revision;
    let wrong = String::from_utf8(raw.clone())
        .unwrap()
        .replace("Frozen body", "Changed body")
        .into_bytes();
    assert!(store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::AppliedSentCopy {
                result: Box::new(result(wrong))
            },
            1007
        )
        .await
        .is_err());
    assert_eq!(store.status().await.unwrap().revision, before);
    let done = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::AppliedSentCopy {
                result: Box::new(result(raw.clone())),
            },
            1007,
        )
        .await
        .unwrap();
    assert_eq!(done.state, "applied");
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
    let attempts = store.operation_attempts(account, op.id).await.unwrap();
    assert_eq!(attempts[0].receipts.len(), 2);
    assert_eq!(attempts[0].receipts[0].kind, "smtp_accepted");
    assert_eq!(attempts[0].receipts[1].kind, "server_sent_observed");
    assert_eq!(attempts[0].receipts[1].source, "positive_read");
    assert_eq!(attempts[1].receipts[0].kind, "sent_copy");
    store.close().await.unwrap();
}
