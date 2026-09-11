#![allow(clippy::unwrap_used, clippy::panic)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        imap::{ImapMailboxState, ImapPlacement, MailboxName},
        imap_account::ImapAccountConfig,
        submission::{freeze, SubmissionTransport},
    },
    store::{
        AttemptKind, AttemptOutcome, ConfirmationEvidence, ConnectedAccount, EnqueueSend,
        ResolutionDecision, ResolveOperation, SaveDraft, SmtpStep, Store,
    },
};
use zeroize::Zeroizing;
#[tokio::test]
async fn smtp_manual_confirmation_is_an_audited_human_decision_without_fabricated_transport_receipts(
) {
    for transmitted in [false, true] {
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
        if transmitted {
            store
                .start_smtp_data(
                    account.clone(),
                    op.id.clone(),
                    1,
                    ImapMailboxState {
                        uid_next: Some(10),
                        ..state
                    },
                    1002,
                )
                .await
                .unwrap();
        }
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
        let placement = ImapPlacement::new(account.parse().unwrap(), mailbox, 1234, 10).unwrap();
        let evidence = ConfirmationEvidence {
        provider_id: placement.provider_id().unwrap(),
        message_id: Some("original@nuncio.invalid".into()),
        etag: None,
        observed_at_ms: 1004,
        note: "I inspected the exact Sent MIME in the named placement".into(),
        smtp_acceptance_note: Some(
            "The independent SMTP acceptance log confirms this Message-ID, sender and recipients"
                .into(),
        ),
    };
        let mut decision = ResolveOperation {
            account_id: account.clone(),
            operation_id: op.id.clone(),
            expected_version: unknown.version,
            decision: ResolutionDecision::ConfirmApplied {
                evidence: evidence.clone(),
            },
        };
        if !transmitted {
            assert!(store
                .resolve_operation(decision.clone(), None, 1005)
                .await
                .is_err());
            let abandoned = store
                .resolve_operation(
                    ResolveOperation {
                        decision: ResolutionDecision::Abandon {
                            reason: "Leave unstarted work unresolved".into(),
                        },
                        ..decision
                    },
                    None,
                    1005,
                )
                .await
                .unwrap();
            assert_eq!(abandoned.state, "uncertain");
            assert_eq!(abandoned.disposition.as_deref(), Some("abandoned"));
            assert_eq!(
                store
                    .smtp_progress(account.clone(), op.id.clone())
                    .await
                    .unwrap()
                    .step,
                SmtpStep::Prepared
            );
            assert!(store
                .operation_attempts(account, op.id)
                .await
                .unwrap()
                .iter()
                .all(|a| a.receipts.is_empty()));
            store.close().await.unwrap();
            continue;
        }
        for invalid in [
            "acceptance_absent",
            "acceptance_empty",
            "acceptance_control",
            "message_id",
            "account",
            "mailbox",
            "epoch",
            "old_uid",
            "zero_uid",
            "etag",
            "before_creation",
            "future",
            "noncanonical_id",
        ] {
            let mut wrong = evidence.clone();
            match invalid {
                "acceptance_absent" => wrong.smtp_acceptance_note = None,
                "acceptance_empty" => wrong.smtp_acceptance_note = Some(" ".into()),
                "acceptance_control" => wrong.smtp_acceptance_note = Some("line\nline".into()),
                "message_id" => wrong.message_id = Some("other@nuncio.invalid".into()),
                "account" => {
                    wrong.provider_id = ImapPlacement::new(
                        uuid::Uuid::new_v4().to_string().parse().unwrap(),
                        mailbox,
                        1234,
                        10,
                    )
                    .unwrap()
                    .provider_id()
                    .unwrap()
                }
                "mailbox" => {
                    wrong.provider_id = ImapPlacement::new(
                        account.parse().unwrap(),
                        uuid::Uuid::new_v4().to_string().parse().unwrap(),
                        1234,
                        10,
                    )
                    .unwrap()
                    .provider_id()
                    .unwrap()
                }
                "epoch" => {
                    wrong.provider_id =
                        ImapPlacement::new(account.parse().unwrap(), mailbox, 1235, 10)
                            .unwrap()
                            .provider_id()
                            .unwrap()
                }
                "old_uid" => {
                    wrong.provider_id =
                        ImapPlacement::new(account.parse().unwrap(), mailbox, 1234, 9)
                            .unwrap()
                            .provider_id()
                            .unwrap()
                }
                "zero_uid" => {
                    let mut value: serde_json::Value =
                        serde_json::from_str(&wrong.provider_id).unwrap();
                    value[2]["uid"] = 0.into();
                    wrong.provider_id = value.to_string();
                }
                "etag" => wrong.etag = Some("http-etag".into()),
                "before_creation" => wrong.observed_at_ms = 1000,
                "future" => wrong.observed_at_ms = 1006,
                "noncanonical_id" => wrong.provider_id.push(' '),
                _ => unreachable!(),
            }
            let before = store.status().await.unwrap().revision;
            assert!(
                store
                    .resolve_operation(
                        ResolveOperation {
                            decision: ResolutionDecision::ConfirmApplied { evidence: wrong },
                            ..decision.clone()
                        },
                        None,
                        1005
                    )
                    .await
                    .is_err(),
                "accepted {invalid}"
            );
            assert_eq!(store.status().await.unwrap().revision, before);
        }
        store
            .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 1005)
            .await
            .unwrap();
        let active = store
            .get_operation(account.clone(), op.id.clone())
            .await
            .unwrap();
        decision.expected_version = active.version;
        assert!(store
            .resolve_operation(decision.clone(), None, 1006)
            .await
            .is_err());
        let unknown = store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                2,
                AttemptOutcome::Uncertain {
                    code: "still_unconfirmed".into(),
                },
                1006,
            )
            .await
            .unwrap();
        assert!(store
            .resolve_operation(decision.clone(), None, 1007)
            .await
            .is_err());
        decision.expected_version = unknown.version;
        let before = store.status().await.unwrap().revision;
        let confirmed = store
            .resolve_operation(decision.clone(), None, 1007)
            .await
            .unwrap();
        assert_eq!(confirmed.state, "applied");
        assert_eq!(confirmed.disposition.as_deref(), Some("manual_confirmed"));
        assert!(!confirmed.needs_reconciliation);
        assert_eq!(confirmed.resolutions.len(), 1);
        assert_eq!(confirmed.resolutions[0].decision, "confirm_applied");
        assert_eq!(store.status().await.unwrap().revision, before + 1);
        assert_eq!(
            store
                .smtp_progress(account.clone(), op.id.clone())
                .await
                .unwrap()
                .step,
            SmtpStep::Started
        );
        let attempts = store
            .operation_attempts(account.clone(), op.id.clone())
            .await
            .unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].outcome.as_deref(), Some("uncertain"));
        assert!(attempts.iter().all(|a| a.receipts.is_empty()));
        store.close().await.unwrap();
        let store = Store::open(temp.path(), key).await.unwrap();
        let again = store.resolve_operation(decision, None, 1008).await.unwrap();
        assert_eq!(again.version, confirmed.version);
        assert_eq!(store.status().await.unwrap().revision, before + 1);
        store.close().await.unwrap();
    }
}
