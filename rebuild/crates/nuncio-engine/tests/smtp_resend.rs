#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        imap::{ImapMailboxState, MailboxName},
        imap_account::ImapAccountConfig,
        submission::{freeze, reidentify, FrozenMessage, SentFingerprint, SubmissionTransport},
    },
    store::{
        AttemptKind, AttemptOutcome, ConnectedAccount, EnqueueSend, ResolutionDecision,
        ResolveOperation, SaveDraft, SmtpStep, Store,
    },
};
use zeroize::Zeroizing;

#[tokio::test]
async fn smtp_resend_atomically_captures_fresh_transport_intent_and_keeps_original_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x72; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let config:ImapAccountConfig=serde_json::from_value(serde_json::json!({"address":"alpha@example.test","imap":{"host":"nas.example.test","port":993,"tls":"implicit","username":"alpha"},"smtp":{"host":"nas.example.test","port":587,"tls":"start_tls","username":"alpha"},"sent_policy":"server","sent_folder":"Sent"})).unwrap();
    store
        .prepare_credential("synthetic-resend-ref".into())
        .await
        .unwrap();
    store
        .connect_imap(
            ConnectedAccount {
                id: account.clone(),
                subject: config.identity().unwrap(),
                address: config.address.clone(),
                credential_ref: "synthetic-resend-ref".into(),
            },
            config.clone(),
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
    let mailbox = store
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
        text: Some("Frozen original body".into()),
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
        "original@nuncio.invalid",
        1000,
        SubmissionTransport::Smtp,
    )
    .unwrap();
    let replacement = FrozenMessage {
        wire: reidentify(&frozen.wire, "replacement@nuncio.invalid", 1010).unwrap(),
        sent_copy: Some(
            reidentify(
                frozen.sent_copy.as_deref().unwrap(),
                "replacement@nuncio.invalid",
                1010,
            )
            .unwrap(),
        ),
        recipients: frozen.recipients.clone(),
        thread_id: None,
        message_id: "replacement@nuncio.invalid".into(),
    };
    let fingerprint =
        SentFingerprint::from_mime(replacement.sent_copy.as_deref().unwrap()).unwrap();
    let op = store
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
            1001,
        )
        .await
        .unwrap();
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 1002)
        .await
        .unwrap();
    store
        .start_smtp_data(account.clone(), op.id.clone(), 1, state, 1002)
        .await
        .unwrap();
    let unknown = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            1,
            AttemptOutcome::Uncertain {
                code: "smtp_acceptance_unknown".into(),
            },
            1003,
        )
        .await
        .unwrap();
    store
        .delete_draft(account.clone(), draft.id, 1)
        .await
        .unwrap();
    let decision = ResolveOperation {
        account_id: account.clone(),
        operation_id: op.id.clone(),
        expected_version: unknown.version,
        decision: ResolutionDecision::Resend {
            request_id: uuid::Uuid::new_v4().to_string(),
            accept_duplicate_risk: true,
            reason: "I accept possible duplicate delivery".into(),
        },
    };
    let before = store.status().await.unwrap().revision;
    let mut invalid = replacement.clone();
    invalid.sent_copy = Some(b"malformed frozen Sent MIME".to_vec());
    assert!(store
        .resolve_operation(decision.clone(), Some(invalid), 1010)
        .await
        .is_err());
    assert_eq!(store.status().await.unwrap().revision, before);
    assert!(store
        .get_operation(account.clone(), op.id.clone())
        .await
        .unwrap()
        .resolutions
        .is_empty());
    let resolved = store
        .resolve_operation(decision.clone(), Some(replacement), 1010)
        .await
        .unwrap();
    let id = resolved.resolutions[0].replacement_id.clone().unwrap();
    assert_ne!(id, op.id);
    let intent = store
        .smtp_intent(account.clone(), id.clone())
        .await
        .unwrap();
    assert_eq!(intent.account_id.to_string(), account);
    assert!(intent.endpoint == config.smtp);
    assert!(intent.sent_policy == config.sent_policy);
    assert_eq!(intent.mailbox_id, mailbox);
    assert_eq!(intent.uid_validity.get(), 1234);
    assert_eq!(intent.uid_next.get(), 1);
    assert!(intent.sent_fingerprint.unwrap().matches(&fingerprint));
    let progress = store
        .smtp_progress(account.clone(), id.clone())
        .await
        .unwrap();
    assert_eq!(progress.step, SmtpStep::Prepared);
    assert_eq!(progress.sent_floor, None);
    assert_eq!(progress.placement, None);
    assert_eq!(
        store
            .smtp_progress(account.clone(), op.id.clone())
            .await
            .unwrap()
            .step,
        SmtpStep::Started
    );
    assert_eq!(resolved.state, "uncertain");
    assert_eq!(resolved.disposition.as_deref(), Some("resend_requested"));
    assert_eq!(store.status().await.unwrap().revision, before + 2);
    let again = store
        .resolve_operation(decision.clone(), None, 1011)
        .await
        .unwrap();
    assert_eq!(again.version, resolved.version);
    assert_eq!(again.resolutions[0].replacement_id, Some(id.clone()));
    assert_eq!(store.status().await.unwrap().revision, before + 2);
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(
        store
            .get_operation(account.clone(), id.clone())
            .await
            .unwrap()
            .state,
        "queued"
    );
    assert_eq!(
        store
            .smtp_progress(account.clone(), id.clone())
            .await
            .unwrap()
            .step,
        SmtpStep::Prepared
    );
    assert_eq!(
        store
            .smtp_intent(account.clone(), id.clone())
            .await
            .unwrap()
            .mailbox_id,
        mailbox
    );
    let receipts = store
        .operation_attempts(account.clone(), op.id)
        .await
        .unwrap();
    assert!(receipts.iter().all(|a| a.receipts.is_empty()));
    let payload = store.send_payload(account.clone(), id).await.unwrap();
    assert_eq!(payload.message_id, "replacement@nuncio.invalid");
    assert_eq!(
        payload.recipients,
        vec!["beta@example.test", "blind@example.test"]
    );
    let raw = store
        .blob_chunk(account.clone(), payload.wire.id, 0)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("Bcc:"));
    let sent = store
        .blob_chunk(account, payload.sent_copy.unwrap().id, 0)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&sent).contains("Bcc:"));
    store.close().await.unwrap();
}
