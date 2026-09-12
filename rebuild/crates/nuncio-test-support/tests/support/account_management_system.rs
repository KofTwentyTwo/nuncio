use super::*;
use std::time::Duration;

async fn enqueue(h: &SystemHarness, account: &str) -> Operation {
    let draft = h
        .mail()
        .save_draft(input(account))
        .await
        .unwrap()
        .into_inner();
    h.mail()
        .send_draft(SendDraftRequest {
            account_id: account.into(),
            draft_id: draft.id,
            expected_version: Some(draft.version),
            request_id: "738c7306-b0ba-4a92-a195-a633c36b3d32".into(),
        })
        .await
        .unwrap()
        .into_inner()
}
async fn applied(h: &SystemHarness, op: &Operation) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let value = h
            .operations()
            .get_operation(OperationRequest {
                account_id: op.account_id.clone(),
                operation_id: op.id.clone(),
            })
            .await
            .unwrap()
            .into_inner();
        if value.state == "applied" {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "operation did not apply: {}",
            value.state
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
#[tokio::test]
async fn paused_or_purged_admitted_work_does_not_dispatch_or_stop_other_accounts() {
    for purge in [false, true] {
        let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
        let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
            .await
            .account_id
            .unwrap();
        let beta = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
            .await
            .account_id
            .unwrap();
        h.arm("operation_before_dispatch").unwrap();
        h.arm("operation_job_finished").unwrap();
        let op = enqueue(&h, &alpha).await;
        h.wait("operation_before_dispatch").await.unwrap();
        h.accounts()
            .set_account_lifecycle(SetAccountLifecycleRequest {
                account_id: alpha.clone(),
                action: if purge {
                    AccountLifecycleAction::Archive
                } else {
                    AccountLifecycleAction::Pause
                }
                .into(),
            })
            .await
            .unwrap();
        if purge {
            let preview = h
                .accounts()
                .preview_account_purge(AccountRequest {
                    account_id: alpha.clone(),
                })
                .await
                .unwrap()
                .into_inner();
            assert_eq!(preview.queued_operations, 1);
            assert_eq!(preview.unresolved_operations, 0);
            h.accounts()
                .purge_account(PurgeAccountRequest {
                    account_id: alpha.clone(),
                    confirm_account_id: alpha.clone(),
                    version: preview.version,
                    revision: preview.revision,
                })
                .await
                .unwrap();
        }
        h.release("operation_before_dispatch").unwrap();
        h.wait("operation_job_finished").await.unwrap();
        assert!(h
            .google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .is_empty());
        if !purge {
            let saved = h
                .operations()
                .get_operation(OperationRequest {
                    account_id: alpha.clone(),
                    operation_id: op.id.clone(),
                })
                .await
                .unwrap()
                .into_inner();
            assert_eq!(saved.state, "queued");
            let attempts = h
                .operations()
                .list_attempts(ListOperationAttemptsRequest {
                    account_id: alpha.clone(),
                    operation_id: op.id.clone(),
                    page_size: 100,
                    page_token: None,
                })
                .await
                .unwrap()
                .into_inner();
            assert!(attempts.items.is_empty());
        }
        h.release("operation_job_finished").unwrap();
        let other = enqueue(&h, &beta).await;
        applied(&h, &other).await;
        assert_eq!(
            h.google
                .control()
                .accepted_sends("beta@example.test")
                .await
                .len(),
            1
        );
        if !purge {
            h.accounts()
                .set_account_lifecycle(SetAccountLifecycleRequest {
                    account_id: alpha,
                    action: AccountLifecycleAction::Resume.into(),
                })
                .await
                .unwrap();
            applied(&h, &op).await;
            assert_eq!(
                h.google
                    .control()
                    .accepted_sends("alpha@example.test")
                    .await
                    .len(),
                1
            );
        }
        h.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn account_purge_preserves_lost_ack_send_evidence_until_explicit_abandonment() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let control = h.google.control();
    control
        .inject(Fault {
            method: "POST".into(),
            path: "/gmail/v1/users/me/messages/send".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::After,
            action: FaultAction::Disconnect,
        })
        .await;
    control
        .inject(Fault {
            method: "GET".into(),
            path: "/gmail/v1/users/me/messages".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 400,
                retry_after_secs: None,
            },
        })
        .await;
    let op = enqueue(&h, &alpha).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let unresolved = loop {
        let value = h
            .operations()
            .get_operation(OperationRequest {
                account_id: alpha.clone(),
                operation_id: op.id.clone(),
            })
            .await
            .unwrap()
            .into_inner();
        if value.state == "uncertain" && !value.needs_reconciliation {
            break value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "uncertainty not retained: {}",
            value.state
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(control.accepted_sends("alpha@example.test").await.len(), 1);
    h.accounts()
        .set_account_lifecycle(SetAccountLifecycleRequest {
            account_id: alpha.clone(),
            action: AccountLifecycleAction::Archive.into(),
        })
        .await
        .unwrap();
    let preview = h
        .accounts()
        .preview_account_purge(AccountRequest {
            account_id: alpha.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(preview.unresolved_operations, 1);
    let error = h
        .accounts()
        .purge_account(PurgeAccountRequest {
            account_id: alpha.clone(),
            confirm_account_id: alpha.clone(),
            version: preview.version,
            revision: preview.revision,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    let evidence = h
        .operations()
        .list_attempts(ListOperationAttemptsRequest {
            account_id: alpha.clone(),
            operation_id: op.id.clone(),
            page_size: 100,
            page_token: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(evidence.items.len(), 2);
    let resend = h
        .operations()
        .resolve_operation(ResolveOperationRequest {
            account_id: alpha.clone(),
            operation_id: op.id.clone(),
            expected_version: unresolved.version,
            decision: Some(resolve_operation_request::Decision::Resend(
                OperationResend {
                    request_id: "785a454a-1205-4484-a752-fd3b2d82fd83".into(),
                    accept_duplicate_risk: true,
                    reason: "Archived account must reject new remote work".into(),
                },
            )),
        })
        .await;
    assert_eq!(
        resend.err().map(|error| error.code()),
        Some(tonic::Code::FailedPrecondition)
    );
    let reconcile = h
        .operations()
        .reconcile_operation(ReconcileOperationRequest {
            account_id: alpha.clone(),
            operation_id: op.id.clone(),
            request_id: "8e338458-062b-4681-95cf-e0c5a5735ecd".into(),
            expected_version: unresolved.version,
            mode: ReconciliationMode::Observe.into(),
        })
        .await;
    assert_eq!(
        reconcile.err().map(|error| error.code()),
        Some(tonic::Code::FailedPrecondition)
    );
    h.operations()
        .resolve_operation(ResolveOperationRequest {
            account_id: alpha.clone(),
            operation_id: op.id,
            expected_version: unresolved.version,
            decision: Some(resolve_operation_request::Decision::Abandon(
                OperationAbandonment {
                    reason: "Keep remote delivery; explicitly discard local history".into(),
                },
            )),
        })
        .await
        .unwrap();
    let preview = h
        .accounts()
        .preview_account_purge(AccountRequest {
            account_id: alpha.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(preview.unresolved_operations, 0);
    h.accounts()
        .purge_account(PurgeAccountRequest {
            account_id: alpha.clone(),
            confirm_account_id: alpha,
            version: preview.version,
            revision: preview.revision,
        })
        .await
        .unwrap();
    assert_eq!(control.accepted_sends("alpha@example.test").await.len(), 1);
    h.shutdown().await.unwrap();
}
