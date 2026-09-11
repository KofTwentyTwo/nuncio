use super::*;
use crate::smtp_resolution_evidence::observe;

#[tokio::test]
async fn smtp_manual_confirmation_and_abandonment_preserve_actual_receipts_and_remote_effects(
) -> Result<(), TestError> {
    for (server_sent, confirm) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut mock = MockMailPlus::start_with_sent_policy(&[], server_sent).await?;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
        let mut config = request(&mock, true, "alpha@example.test").await?;
        config.config.as_mut().unwrap().sent_policy = if server_sent {
            v2::SentPolicy::Server
        } else {
            v2::SentPolicy::ClientAppend
        }
        .into();
        let account = h
            .accounts()
            .connect_imap(config)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id;
        sync(&h, &mut mock, &account, true).await?;
        let draft = h.mail().save_draft(v2::SaveDraftRequest {account_id:account.clone(),content:Some(serde_json::from_value(json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"blind@example.test"}],"subject":"SMTP resolution evidence","text":"Independent acceptance and copy\n.leading dot\n"}))?),..Default::default()}).await?.into_inner();
        mock.control(json!({"command":"inject","name":"lost-ack","protocol":if server_sent {"smtp"}else{"imap"},"verb":if server_sent {"DATA"}else{"APPEND"},"phase":"after","action":"disconnect"})).await?;
        let send = v2::SendDraftRequest {
            account_id: account.clone(),
            draft_id: draft.id,
            request_id: "bed5822d-d7a0-4b06-aa5a-2d1b6d501ded".into(),
            expected_version: Some(draft.version),
        };
        let queued = h.mail().send_draft(send.clone()).await?.into_inner();
        let get = v2::OperationRequest {
            account_id: account.clone(),
            operation_id: queued.id.clone(),
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
        let unknown = loop {
            let op = h
                .operations()
                .get_operation(get.clone())
                .await?
                .into_inner();
            if op.state == "uncertain" && !op.needs_reconciliation {
                break op;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{} {:?}",
                op.state,
                op.error_code
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(
            unknown.error_code.as_deref(),
            Some(if server_sent {
                "smtp_acceptance_unknown"
            } else {
                "imap_sent_copy_identity_unknown"
            })
        );
        if server_sent {
            mock.control(json!({"command":"wait_server_sent","count":1}))
                .await?;
        }
        let observed = observe(&mut mock, server_sent).await?;
        let desired: serde_json::Value = serde_json::from_str(&unknown.desired_state_json)?;
        assert_eq!(desired["message_id"], observed.message_id);
        sync(&h, &mut mock, &account, false).await?;
        assert_eq!(
            h.operations()
                .get_operation(get.clone())
                .await?
                .into_inner(),
            unknown
        );
        let local = list(&h, &account).await?;
        let local = local
            .items
            .iter()
            .find(|m| m.subject.as_deref() == Some("SMTP resolution evidence"))
            .unwrap();
        observed.check_placement(&local.provider_id, &account)?;
        let attempts_request = v2::ListOperationAttemptsRequest {
            account_id: account.clone(),
            operation_id: queued.id.clone(),
            page_size: 100,
            page_token: None,
        };
        let before = h
            .operations()
            .list_attempts(attempts_request.clone())
            .await?
            .into_inner();
        let receipts = before
            .items
            .iter()
            .flat_map(|a| &a.receipts)
            .collect::<Vec<_>>();
        assert_eq!(
            receipts
                .iter()
                .filter(|r| r.kind == "smtp_accepted")
                .count(),
            usize::from(!server_sent)
        );
        assert!(receipts
            .iter()
            .all(|r| r.kind != "sent_copy" && r.kind != "server_sent_observed"));
        let evidence = v2::OperationConfirmation {
            provider_id: local.provider_id.clone(),
            message_id: Some(observed.message_id),
            etag: None,
            observed_at_ms: unknown.updated_at_ms,
            note: "Independently read exact Sent UID, epoch, Message-ID and MIME".into(),
            smtp_acceptance_note: Some(
                "Independent Mailpit acceptance capture and SMTP envelope observed".into(),
            ),
        };
        let mut decision = v2::ResolveOperationRequest {
            account_id: account.clone(),
            operation_id: queued.id.clone(),
            expected_version: unknown.version,
            decision: Some(v2::resolve_operation_request::Decision::ConfirmApplied(
                evidence.clone(),
            )),
        };
        assert_eq!(
            h.anonymous_operations()
                .resolve_operation(decision.clone())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unauthenticated
        );
        let mut bad = evidence.clone();
        bad.smtp_acceptance_note = None;
        let mut invalid = decision.clone();
        invalid.decision = Some(v2::resolve_operation_request::Decision::ConfirmApplied(bad));
        assert_eq!(
            h.operations()
                .resolve_operation(invalid)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
        let mut bad = evidence;
        bad.message_id = Some("unrelated@example.test".into());
        let mut invalid = decision.clone();
        invalid.decision = Some(v2::resolve_operation_request::Decision::ConfirmApplied(bad));
        assert_eq!(
            h.operations()
                .resolve_operation(invalid)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
        if !confirm {
            decision.decision = Some(v2::resolve_operation_request::Decision::Abandon(
                v2::OperationAbandonment {
                    reason: "Stop reconciliation while preserving uncertain outcome".into(),
                },
            ));
        }
        let mut one = h.operations();
        let mut two = h.operations();
        let (first, second) = tokio::join!(
            one.resolve_operation(decision.clone()),
            two.resolve_operation(decision.clone())
        );
        let resolved = first?.into_inner();
        assert_eq!(resolved, second?.into_inner());
        assert_eq!(
            resolved.state,
            if confirm { "applied" } else { "uncertain" }
        );
        assert_eq!(
            resolved.disposition.as_deref(),
            Some(if confirm {
                "manual_confirmed"
            } else {
                "abandoned"
            })
        );
        assert_eq!(
            resolved.error_code,
            if confirm { None } else { unknown.error_code }
        );
        assert_eq!(resolved.resolutions.len(), 1);
        assert!(!resolved.needs_reconciliation);
        h.shutdown().await?;
        h.restart().await?;
        assert_eq!(
            h.operations()
                .resolve_operation(decision.clone())
                .await?
                .into_inner(),
            resolved
        );
        assert_eq!(h.mail().send_draft(send).await?.into_inner().id, queued.id);
        let after = h
            .operations()
            .list_attempts(attempts_request)
            .await?
            .into_inner();
        assert_eq!(before.items, after.items);
        decision.expected_version = resolved.version;
        decision.decision = Some(v2::resolve_operation_request::Decision::Abandon(
            v2::OperationAbandonment {
                reason: "Conflicting later decision".into(),
            },
        ));
        assert_eq!(
            h.operations()
                .resolve_operation(decision)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::FailedPrecondition
        );
        assert_eq!(observe(&mut mock, server_sent).await?.raw, observed.raw);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
