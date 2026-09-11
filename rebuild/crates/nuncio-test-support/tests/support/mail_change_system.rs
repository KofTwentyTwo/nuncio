use super::*;
use nuncio_test_support::google::{Fault, FaultAction, Phase};

#[tokio::test]
async fn mail_mutations_use_authenticated_scoped_intents_and_reconcile_lost_responses() {
    for (phase, action, expected) in [
        (
            Phase::Before,
            FaultAction::Status {
                code: 400,
                retry_after_secs: None,
            },
            "failed",
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 404,
                retry_after_secs: None,
            },
            "conflict",
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 401,
                retry_after_secs: None,
            },
            "applied",
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 429,
                retry_after_secs: Some(0),
            },
            "applied",
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 503,
                retry_after_secs: None,
            },
            "applied",
        ),
        (Phase::After, FaultAction::Disconnect, "applied"),
        (Phase::After, FaultAction::MalformedJson, "applied"),
        (Phase::After, FaultAction::TruncatedBody, "applied"),
        (
            Phase::After,
            FaultAction::Status {
                code: 503,
                retry_after_secs: None,
            },
            "applied",
        ),
    ] {
        let after = phase == Phase::After;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
        let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
            .await
            .account_id
            .unwrap();
        let other = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
            .await
            .account_id
            .unwrap();
        let mut run = h
            .authenticated()
            .start_sync(StartSyncRequest {
                account_id: account.clone(),
                full: false,
                fetch_message_id: None,
            })
            .await
            .unwrap()
            .into_inner();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        while matches!(run.state.as_str(), "queued" | "running") {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            run = h
                .authenticated()
                .get_sync_run(SyncRunRequest {
                    account_id: account.clone(),
                    run_id: run.id,
                })
                .await
                .unwrap()
                .into_inner();
        }
        assert_eq!(run.state, "succeeded");
        let messages = h
            .mail()
            .list_messages(ListMailRequest {
                account_id: account.clone(),
                page_size: 100,
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        let message = messages
            .items
            .iter()
            .find(|m| m.provider_id == "m-001")
            .unwrap();
        let request = ChangeMessageRequest {
            account_id: account.clone(),
            message_id: message.id.clone(),
            request_id: "cd5263af-cd09-4cc5-8d07-d963996cd1ce".into(),
            action: Some(change_message_request::Action::Archive(
                MailArchiveChange {},
            )),
        };
        assert_eq!(
            h.anonymous_mail()
                .change_message(request.clone())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unauthenticated
        );
        assert_eq!(
            h.anonymous_mail()
                .get_capabilities(MailCapabilitiesRequest {
                    account_id: account.clone()
                })
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unauthenticated
        );
        let mut cross = request.clone();
        cross.account_id = other;
        assert_eq!(
            h.mail().change_message(cross).await.unwrap_err().code(),
            tonic::Code::NotFound
        );
        let mut missing = request.clone();
        missing.action = None;
        assert_eq!(
            h.mail().change_message(missing).await.unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
        let mut invalid_label = request.clone();
        invalid_label.action = Some(change_message_request::Action::Label(MailLabelChange {
            collection_id: message
                .collections
                .iter()
                .find(|c| c.provider_id == "INBOX")
                .unwrap()
                .id
                .clone(),
            present: false,
        }));
        assert_eq!(
            h.mail()
                .change_message(invalid_label)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::NotFound,
            "system labels require their typed action"
        );
        h.google
            .control()
            .inject(Fault {
                method: "POST".into(),
                path: "/gmail/v1/users/me/messages/m-001/modify".into(),
                account: Some("alpha@example.test".into()),
                call: None,
                phase,
                action,
            })
            .await;
        let mut op = h
            .mail()
            .change_message(request.clone())
            .await
            .unwrap()
            .into_inner();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        while op.state != expected {
            assert!(
                tokio::time::Instant::now() < deadline,
                "expected {expected}, got {} {:?}",
                op.state,
                op.error_code
            );
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            op = h
                .operations()
                .get_operation(OperationRequest {
                    account_id: account.clone(),
                    operation_id: op.id,
                })
                .await
                .unwrap()
                .into_inner();
        }
        let remote = h.google.control().snapshot().await;
        let labels = &remote.mail["alpha@example.test"].messages["m-001"].labels;
        assert!(labels.contains("Label_project") && labels.contains("UNREAD"));
        assert_eq!(labels.contains("INBOX"), expected != "applied");
        assert!(remote.mail["beta@example.test"].messages["m-001"]
            .labels
            .contains("INBOX"));
        assert_eq!(remote.mail["alpha@example.test"].accepted_sends.len(), 0);
        let local = h
            .mail()
            .get_message(MessageRequest {
                account_id: account.clone(),
                message_id: message.id.clone(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            local
                .message
                .unwrap()
                .collections
                .into_iter()
                .map(|c| c.provider_id)
                .collect::<std::collections::BTreeSet<_>>(),
            *labels
        );
        if after {
            let attempts = h
                .operations()
                .list_attempts(ListOperationAttemptsRequest {
                    account_id: account.clone(),
                    operation_id: op.id.clone(),
                    page_size: 100,
                    page_token: None,
                })
                .await
                .unwrap()
                .into_inner();
            assert_eq!(attempts.items.len(), 2);
            assert_eq!(attempts.items[0].kind, "reconcile");
            assert_eq!(attempts.items[0].receipts[0].source, "positive_read");
            assert_eq!(
                remote
                    .requests
                    .iter()
                    .filter(|r| r.method == "POST" && r.path.ends_with("/m-001/modify"))
                    .map(|r| r.count)
                    .sum::<u64>(),
                1
            );
        }
        assert_eq!(
            h.mail()
                .change_message(request.clone())
                .await
                .unwrap()
                .into_inner()
                .id,
            op.id
        );
        let mut changed = request;
        changed.action = Some(change_message_request::Action::Read(MailReadChange {
            read: true,
        }));
        assert_eq!(
            h.mail().change_message(changed).await.unwrap_err().code(),
            tonic::Code::FailedPrecondition
        );
        h.shutdown().await.unwrap();
    }
}
