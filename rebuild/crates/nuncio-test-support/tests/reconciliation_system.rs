#![allow(clippy::unwrap_used)]
#[path = "support/reconciliation_mutations_system.rs"]
mod reconciliation_mutations_system;
pub mod support;
use nuncio_proto::v2::*;
use nuncio_test_support::google::{Fault, FaultAction, Phase, Seed};
#[path = "support/backup_system.rs"]
mod backup_system;
use backup_system::{backup, upload};
use support::{
    auth::{begin, finish},
    system::SystemHarness,
};

async fn settled(h: &SystemHarness, account: &str, id: &str, state: &str) -> Operation {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let op = h
                .operations()
                .get_operation(OperationRequest {
                    account_id: account.into(),
                    operation_id: id.into(),
                })
                .await
                .unwrap()
                .into_inner();
            if !op.needs_reconciliation
                && matches!(
                    op.state.as_str(),
                    "applied" | "failed" | "conflict" | "uncertain"
                )
            {
                assert_eq!(op.state, state, "unexpected outcome: {:?}", op.error_code);
                return op;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn restored_google_send_reconciles_only_unique_positive_evidence_with_durable_api_identity() {
    for accepted_after_snapshot in [false, true] {
        let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
        let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
            .await
            .account_id
            .unwrap();
        let draft=h.mail().save_draft(SaveDraftRequest{account_id:account.clone(),draft_id:None,expected_version:None,content:Some(serde_json::from_value(serde_json::json!({"to":[{"address":"recipient@example.test"}],"subject":"Reconcile retained submission","text":"Immutable original body"})).unwrap())}).await.unwrap().into_inner();
        h.arm("operation_before_dispatch").unwrap();
        let queued = h
            .mail()
            .send_draft(SendDraftRequest {
                account_id: account.clone(),
                draft_id: draft.id,
                request_id: "1a4428d0-53d3-41a8-a9ed-6efafc8b8332".into(),
                expected_version: Some(1),
            })
            .await
            .unwrap()
            .into_inner();
        h.wait("operation_before_dispatch").await.unwrap();
        let bytes = backup(&h).await;
        assert!(h
            .google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .is_empty());
        if accepted_after_snapshot {
            h.google
                .control()
                .inject(Fault {
                    method: "POST".into(),
                    path: "/gmail/v1/users/me/messages/send".into(),
                    account: Some("alpha@example.test".into()),
                    call: None,
                    phase: Phase::After,
                    action: FaultAction::Disconnect,
                })
                .await;
            h.release("operation_before_dispatch").unwrap();
            settled(&h, &account, &queued.id, "applied").await;
        }
        let restored = h
            .maintenance()
            .restore_backup(upload(&bytes))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(restored.held_operations, 1);
        h.shutdown().await.unwrap();
        h.directory = restored.directory.into();
        h.restart().await.unwrap();
        let held = settled(&h, &account, &queued.id, "uncertain").await;
        assert_eq!(
            finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
                .await
                .account_id
                .as_deref(),
            Some(account.as_str())
        );
        let before = h.google.control().snapshot().await;
        let input = ReconcileOperationRequest {
            account_id: account.clone(),
            operation_id: queued.id.clone(),
            request_id: "d4e86240-ad28-4bf6-8b0b-1b2a3b47c2b5".into(),
            expected_version: held.version,
            mode: ReconciliationMode::Observe as i32,
        };
        assert_eq!(
            h.anonymous_operations()
                .reconcile_operation(input.clone())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unauthenticated
        );
        for mode in [0, 99] {
            let mut invalid = input.clone();
            invalid.mode = mode;
            assert_eq!(
                h.operations()
                    .reconcile_operation(invalid)
                    .await
                    .unwrap_err()
                    .code(),
                tonic::Code::InvalidArgument
            );
        }
        let mut invalid = input.clone();
        invalid.expected_version = 0;
        assert_eq!(
            h.operations()
                .reconcile_operation(invalid)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
        let mut invalid = input.clone();
        invalid.account_id = "a9d5f357-5f92-410e-97b1-c9f1f89970ba".into();
        assert_eq!(
            h.operations()
                .reconcile_operation(invalid)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::NotFound
        );
        h.arm("operation_after_attempt").unwrap();
        let admitted = h
            .operations()
            .reconcile_operation(input.clone())
            .await
            .unwrap()
            .into_inner();
        assert!(admitted.needs_reconciliation);
        h.wait("operation_after_attempt").await.unwrap();
        let replay = h
            .operations()
            .reconcile_operation(input.clone())
            .await
            .unwrap()
            .into_inner();
        assert_eq!(replay.reconciliation.unwrap().request_id, input.request_id);
        let mut conflict = input.clone();
        conflict.mode = ReconciliationMode::ResumeSafe as i32;
        assert_eq!(
            h.operations()
                .reconcile_operation(conflict)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::FailedPrecondition
        );
        h.release("operation_after_attempt").unwrap();
        let observed = settled(
            &h,
            &account,
            &queued.id,
            if accepted_after_snapshot {
                "applied"
            } else {
                "uncertain"
            },
        )
        .await;
        assert_eq!(observed.request_id, queued.request_id);
        assert_eq!(
            observed
                .reconciliation
                .as_ref()
                .unwrap()
                .first_attempt_ordinal,
            Some(1)
        );
        let attempts = h
            .operations()
            .list_attempts(ListOperationAttemptsRequest {
                account_id: account.clone(),
                operation_id: queued.id.clone(),
                page_size: 100,
                page_token: None,
            })
            .await
            .unwrap()
            .into_inner()
            .items;
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].kind, "reconcile");
        assert_eq!(
            attempts[0].receipts.len(),
            usize::from(accepted_after_snapshot)
        );
        if accepted_after_snapshot {
            assert_eq!(attempts[0].receipts[0].source, "positive_read");
            assert_eq!(attempts[0].receipts[0].kind, "google_send");
        } else {
            assert_eq!(
                observed.error_code.as_deref(),
                Some("no_positive_send_evidence")
            );
        }
        let after = h.google.control().snapshot().await;
        assert_eq!(
            serde_json::to_value(after.mail).unwrap(),
            serde_json::to_value(before.mail).unwrap()
        );
        assert_eq!(
            serde_json::to_value(after.calendars).unwrap(),
            serde_json::to_value(before.calendars).unwrap()
        );
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            usize::from(accepted_after_snapshot)
        );
        h.shutdown().await.unwrap();
        h.restart().await.unwrap();
        assert_eq!(
            serde_json::to_value(
                h.operations()
                    .reconcile_operation(input)
                    .await
                    .unwrap()
                    .into_inner()
            )
            .unwrap(),
            serde_json::to_value(&observed).unwrap()
        );
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            usize::from(accepted_after_snapshot)
        );
        h.shutdown().await.unwrap();
    }
}
