#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        submission::{freeze, reidentify, FrozenMessage, SubmissionTransport},
    },
    store::{
        AccountRecord, AttemptKind, AttemptOutcome, ConfirmationEvidence, EnqueueSend, Operation,
        ResolutionDecision, ResolveOperation, SaveDraft, Store,
    },
};
use zeroize::Zeroizing;

struct Fixture {
    store: Store,
    account: String,
    op: Operation,
    _temp: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path(), Zeroizing::new(vec![0x76; 32]))
            .await
            .unwrap();
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
            text: Some("Frozen body original@nuncio.invalid".into()),
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
            SubmissionTransport::Gmail,
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
        let op = store
            .finish_operation_attempt(
                account.clone(),
                op.id,
                1,
                AttemptOutcome::Uncertain {
                    code: "acknowledgement_lost".into(),
                },
                1003,
            )
            .await
            .unwrap();
        store
            .delete_draft(account.clone(), draft.id, 1)
            .await
            .unwrap();
        Self {
            store,
            account,
            op,
            _temp: temp,
        }
    }
    fn request(&self, decision: ResolutionDecision) -> ResolveOperation {
        ResolveOperation {
            account_id: self.account.clone(),
            operation_id: self.op.id.clone(),
            expected_version: self.op.version,
            decision,
        }
    }
}

#[tokio::test]
async fn abandonment_retains_unknown_outcome_and_audits_one_idempotent_versioned_decision() {
    let f = Fixture::new().await;
    let request = f.request(ResolutionDecision::Abandon {
        reason: "Do not attempt further work".into(),
    });
    let mut invalid = request.clone();
    invalid.expected_version += 1;
    let before = f.store.status().await.unwrap().revision;
    assert!(f
        .store
        .resolve_operation(invalid, None, 1005)
        .await
        .is_err());
    let mut invalid = request.clone();
    invalid.account_id = uuid::Uuid::new_v4().to_string();
    assert!(f
        .store
        .resolve_operation(invalid, None, 1005)
        .await
        .is_err());
    assert_eq!(f.store.status().await.unwrap().revision, before);
    let abandoned = f
        .store
        .resolve_operation(request.clone(), None, 1005)
        .await
        .unwrap();
    assert_eq!(abandoned.state, "uncertain");
    assert_eq!(abandoned.disposition.as_deref(), Some("abandoned"));
    assert!(!abandoned.needs_reconciliation);
    assert_eq!(abandoned.resolutions.len(), 1);
    assert_eq!(abandoned.resolutions[0].decision, "abandon");
    assert_eq!(abandoned.resolutions[0].expected_version, f.op.version);
    assert_eq!(f.store.status().await.unwrap().revision, before + 1);
    assert_eq!(
        f.store
            .resolve_operation(request, None, 1006)
            .await
            .unwrap()
            .version,
        abandoned.version
    );
    assert_eq!(f.store.status().await.unwrap().revision, before + 1);
    assert!(f
        .store
        .resolve_operation(
            f.request(ResolutionDecision::Abandon {
                reason: "Different decision text".into()
            }),
            None,
            1006
        )
        .await
        .is_err());
    assert!(f
        .store
        .begin_operation_attempt(
            f.account.clone(),
            f.op.id.clone(),
            AttemptKind::Reconcile,
            1006
        )
        .await
        .is_err());
    assert!(f
        .store
        .cancel_operation(f.account, f.op.id, 1006)
        .await
        .is_err());
    f.store.close().await.unwrap();
}

#[tokio::test]
async fn manual_confirmation_requires_matching_positive_evidence_and_cannot_race_active_reconciliation(
) {
    let f = Fixture::new().await;
    let evidence = ConfirmationEvidence {
        provider_id: "remote-accepted".into(),
        message_id: Some("original@nuncio.invalid".into()),
        etag: None,
        observed_at_ms: 1004,
        note: "I opened the accepted Sent message and checked its complete Message-ID".into(),
        smtp_acceptance_note: None,
    };
    assert!(
        serde_json::to_value(&evidence)
            .unwrap()
            .get("smtp_acceptance_note")
            .is_none(),
        "legacy Google resolution canonical form must not change"
    );
    let mut mixed = evidence.clone();
    mixed.smtp_acceptance_note = Some("SMTP receipt".into());
    assert!(f
        .store
        .resolve_operation(
            f.request(ResolutionDecision::ConfirmApplied { evidence: mixed }),
            None,
            1005
        )
        .await
        .is_err());
    let mut wrong = evidence.clone();
    wrong.message_id = Some("unrelated@nuncio.invalid".into());
    assert!(f
        .store
        .resolve_operation(
            f.request(ResolutionDecision::ConfirmApplied { evidence: wrong }),
            None,
            1005
        )
        .await
        .is_err());
    f.store
        .begin_operation_attempt(
            f.account.clone(),
            f.op.id.clone(),
            AttemptKind::Reconcile,
            1005,
        )
        .await
        .unwrap();
    let active = f
        .store
        .get_operation(f.account.clone(), f.op.id.clone())
        .await
        .unwrap();
    let mut q = f.request(ResolutionDecision::ConfirmApplied {
        evidence: evidence.clone(),
    });
    q.expected_version = active.version;
    assert!(f.store.resolve_operation(q, None, 1006).await.is_err());
    let unknown = f
        .store
        .finish_operation_attempt(
            f.account.clone(),
            f.op.id.clone(),
            2,
            AttemptOutcome::Uncertain {
                code: "no_positive_evidence".into(),
            },
            1006,
        )
        .await
        .unwrap();
    let mut q = f.request(ResolutionDecision::ConfirmApplied { evidence });
    q.expected_version = unknown.version;
    let applied = f.store.resolve_operation(q, None, 1007).await.unwrap();
    assert_eq!(applied.state, "applied");
    assert_eq!(applied.disposition.as_deref(), Some("manual_confirmed"));
    assert_eq!(
        applied.resolutions[0].evidence["decision"]["evidence"]["provider_id"],
        "remote-accepted"
    );
    let attempts = f
        .store
        .operation_attempts(f.account, f.op.id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(
        attempts
            .iter()
            .all(|a| a.outcome.as_deref() == Some("uncertain") && a.receipts.is_empty()),
        "manual confirmation must not rewrite the actual provider attempts"
    );
    f.store.close().await.unwrap();
}

#[tokio::test]
async fn explicit_resend_preserves_frozen_content_after_draft_deletion_and_atomically_links_a_fresh_identity(
) {
    let f = Fixture::new().await;
    let payload = f
        .store
        .send_payload(f.account.clone(), f.op.id.clone())
        .await
        .unwrap();
    let original = f
        .store
        .blob_chunk(f.account.clone(), payload.wire.id, 0)
        .await
        .unwrap();
    let frozen = FrozenMessage {
        wire: reidentify(&original, "replacement@nuncio.invalid", 1005).unwrap(),
        sent_copy: None,
        recipients: payload.recipients,
        thread_id: payload.thread_id,
        message_id: "replacement@nuncio.invalid".into(),
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    let q = f.request(ResolutionDecision::Resend {
        request_id: request_id.clone(),
        accept_duplicate_risk: true,
        reason: "I accept possible duplicate delivery".into(),
    });
    let bad = f.request(ResolutionDecision::Resend {
        request_id: request_id.clone(),
        accept_duplicate_risk: false,
        reason: "I accept possible duplicate delivery".into(),
    });
    assert!(f
        .store
        .resolve_operation(bad, Some(frozen.clone()), 1005)
        .await
        .is_err());
    assert!(f
        .store
        .resolve_operation(q.clone(), None, 1005)
        .await
        .is_err());
    let before = f.store.status().await.unwrap().revision;
    let resolved = f
        .store
        .resolve_operation(q.clone(), Some(frozen), 1005)
        .await
        .unwrap();
    assert_eq!(resolved.state, "uncertain");
    assert_eq!(resolved.disposition.as_deref(), Some("resend_requested"));
    let id = resolved.resolutions[0].replacement_id.clone().unwrap();
    assert_ne!(id, f.op.id);
    let replacement = f
        .store
        .get_operation(f.account.clone(), id.clone())
        .await
        .unwrap();
    assert_eq!(replacement.request_id, request_id);
    assert_eq!(replacement.state, "queued");
    let payload = f.store.send_payload(f.account.clone(), id).await.unwrap();
    assert_eq!(payload.message_id, "replacement@nuncio.invalid");
    let new = f
        .store
        .blob_chunk(f.account.clone(), payload.wire.id, 0)
        .await
        .unwrap();
    let body = |b: &[u8]| b.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert_eq!(&new[body(&new)..], &original[body(&original)..]);
    assert_eq!(f.store.status().await.unwrap().revision, before + 2);
    let retry = f.store.resolve_operation(q, None, 1006).await.unwrap();
    assert_eq!(retry.version, resolved.version);
    assert_eq!(
        retry.resolutions[0].replacement_id,
        resolved.resolutions[0].replacement_id
    );
    assert_eq!(f.store.status().await.unwrap().revision, before + 2);
    assert_eq!(
        f.store
            .query_operations(f.account, 100, None)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    f.store.close().await.unwrap();
}
