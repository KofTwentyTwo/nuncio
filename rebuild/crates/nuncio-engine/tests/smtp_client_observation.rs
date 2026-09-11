#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        identity::AccountId,
        imap::{ImapFlags, ImapMailboxState, ImapPlacement, MailboxName},
        imap_account::ImapAccountConfig,
        mail::decode_mime,
        submission::{freeze, SubmissionTransport},
    },
    store::{
        AttemptKind, AttemptOutcome, ConnectedAccount, EnqueueSend, MailQuery, ReconcileOperation,
        ReconciliationMode, SaveDraft, SentCopyResult, StagedMail, Store,
    },
};
use zeroize::Zeroizing;
#[tokio::test]
async fn client_sent_observation_commits_only_complete_scoped_proof_atomically() {
    for scenario in [
        "accepted",
        "appending",
        "no_request",
        "missing_ack",
        "missing_floor",
        "before_floor",
        "wrong_epoch",
        "wrong_account",
        "wrong_provider",
        "missing_bcc",
        "wrong_mailbox",
        "started",
    ] {
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
        let raw = frozen.sent_copy.clone().unwrap();
        let wire = frozen.wire.clone();
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
        store
            .start_smtp_data(
                account.clone(),
                op.id.clone(),
                1,
                ImapMailboxState {
                    uid_next: Some(10),
                    ..state.clone()
                },
                1003,
            )
            .await
            .unwrap();
        store
            .record_smtp_acceptance(account.clone(), op.id.clone(), 1, None, 1004)
            .await
            .unwrap();
        if scenario == "appending" {
            store
                .start_sent_append(account.clone(), op.id.clone(), 1, 1005)
                .await
                .unwrap();
        }
        store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                1,
                AttemptOutcome::Uncertain {
                    code: "interrupted".into(),
                },
                1006,
            )
            .await
            .unwrap();
        store
            .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 1006)
            .await
            .unwrap();
        let held = store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                2,
                AttemptOutcome::Uncertain {
                    code: "needs_manual_observation".into(),
                },
                1006,
            )
            .await
            .unwrap();
        store
            .request_reconciliation(
                ReconcileOperation {
                    account_id: account.clone(),
                    operation_id: op.id.clone(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    expected_version: held.version,
                    mode: ReconciliationMode::Observe,
                },
                1007,
            )
            .await
            .unwrap();
        store
            .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 1008)
            .await
            .unwrap();
        store.close().await.unwrap();
        {
            let c = rusqlite::Connection::open(temp.path().join("store.db")).unwrap();
            c.pragma_update(None, "key", format!("x'{}'", hex::encode(key.as_slice())))
                .unwrap();
            let sql = match scenario {
                "no_request" => Some("DELETE FROM operation_reconciliation_requests"),
                "missing_ack" => Some("DELETE FROM operation_receipts"),
                "missing_floor" => Some("UPDATE smtp_submissions SET sent_floor=NULL"),
                "started" => Some("UPDATE smtp_submissions SET step='started'"),
                _ => None,
            };
            if let Some(sql) = sql {
                c.execute(sql, []).unwrap();
            }
        }
        let store = Store::open(temp.path(), key.clone()).await.unwrap();
        let placement = ImapPlacement::new(
            if scenario == "wrong_account" {
                AccountId::generate()
            } else {
                account.parse().unwrap()
            },
            folder,
            if scenario == "wrong_epoch" {
                1235
            } else {
                1234
            },
            if scenario == "before_floor" { 9 } else { 10 },
        )
        .unwrap();
        let bytes = if scenario == "missing_bcc" { wire } else { raw };
        let observed = SentCopyResult {
            placement,
            mailbox: ImapMailboxState {
                name: MailboxName::from_unicode(if scenario == "wrong_mailbox" {
                    "INBOX"
                } else {
                    "Sent"
                })
                .unwrap(),
                uid_next: Some(11),
                ..state.clone()
            },
            flags: ImapFlags::new(vec!["\\Seen".into()]).unwrap(),
            mail: StagedMail {
                provider_id: if scenario == "wrong_provider" {
                    "other".into()
                } else {
                    placement.provider_id().unwrap()
                },
                thread_id: None,
                history_id: None,
                internal_date_ms: Some(1004),
                provider_json: "{}".into(),
                subject: None,
                headers: vec![],
                labels: vec![],
                decoded: Some(decode_mime(&bytes, 64 * 1024 * 1024).unwrap()),
                raw: Some(bytes),
                availability: "available".into(),
            },
        };
        let progress = store
            .smtp_progress(account.clone(), op.id.clone())
            .await
            .unwrap();
        let revision = store.status().await.unwrap().revision;
        let history = serde_json::to_value(
            store
                .operation_attempts(account.clone(), op.id.clone())
                .await
                .unwrap(),
        )
        .unwrap();
        let result = store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                3,
                AttemptOutcome::ObservedClientSent {
                    result: Box::new(observed),
                },
                1009,
            )
            .await;
        let success = matches!(scenario, "accepted" | "appending");
        assert_eq!(result.is_ok(), success, "{scenario}");
        if success {
            assert_eq!(result.unwrap().state, "applied");
            let history = store
                .operation_attempts(account.clone(), op.id.clone())
                .await
                .unwrap();
            assert_eq!(history[0].receipts.len(), 1);
            assert_eq!(history[0].receipts[0].kind, "smtp_accepted");
            assert_eq!(history[0].receipts[0].source, "acknowledgement");
            assert_eq!(history[2].receipts.len(), 1);
            assert_eq!(history[2].receipts[0].kind, "sent_copy");
            assert_eq!(history[2].receipts[0].source, "positive_read");
        } else {
            assert_eq!(
                store.status().await.unwrap().revision,
                revision,
                "{scenario}"
            );
            assert_eq!(
                store
                    .smtp_progress(account.clone(), op.id.clone())
                    .await
                    .unwrap(),
                progress,
                "{scenario}"
            );
            assert_eq!(
                serde_json::to_value(
                    store
                        .operation_attempts(account.clone(), op.id.clone())
                        .await
                        .unwrap()
                )
                .unwrap(),
                history,
                "{scenario}"
            );
        }
        assert_eq!(
            store
                .query_mail(MailQuery {
                    account_id: account,
                    collection_id: None,
                    query: None,
                    page_size: 100,
                    page_token: None
                })
                .await
                .unwrap()
                .items
                .len(),
            usize::from(success)
        );
        store.close().await.unwrap();
    }
}
