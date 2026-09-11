use super::*;

#[tokio::test]
async fn smtp_resend_is_account_scoped_idempotent_and_requires_an_explicit_duplicate_risk_decision(
) -> Result<(), TestError> {
    for server_sent in [false, true] {
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
        let beta = h
            .accounts()
            .connect_imap(request(&mock, false, "beta@example.test").await?)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id;
        sync(&h, &mut mock, &account, true).await?;
        let draft=h.mail().save_draft(v2::SaveDraftRequest {account_id:account.clone(),draft_id:None,expected_version:None,content:Some(serde_json::from_value(json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"blind@example.test"}],"subject":"Resend system","text":"Exact frozen body"}))?)}).await?.into_inner();
        mock.control(json!({"command":"inject","name":"lost-first","protocol":"smtp","verb":"DATA","phase":"after","action":"disconnect"})).await?;
        let original = h
            .mail()
            .send_draft(v2::SendDraftRequest {
                account_id: account.clone(),
                draft_id: draft.id.clone(),
                request_id: "6aa83b77-5b9f-4683-a8eb-9c63b30bda45".into(),
                expected_version: Some(draft.version),
            })
            .await?
            .into_inner();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
        let unknown = loop {
            let op = h
                .operations()
                .get_operation(v2::OperationRequest {
                    account_id: account.clone(),
                    operation_id: original.id.clone(),
                })
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
            Some("smtp_acceptance_unknown")
        );
        if server_sent {
            mock.control(json!({"command":"wait_server_sent","count":1}))
                .await?;
        }
        h.mail()
            .delete_draft(v2::DeleteDraftRequest {
                account_id: account.clone(),
                draft_id: draft.id,
                expected_version: draft.version,
            })
            .await?;
        let mut decision = v2::ResolveOperationRequest {
            account_id: account.clone(),
            operation_id: original.id.clone(),
            expected_version: unknown.version,
            decision: Some(v2::resolve_operation_request::Decision::Resend(
                v2::OperationResend {
                    request_id: "e9090828-e447-4253-a251-d97fc269b348".into(),
                    accept_duplicate_risk: false,
                    reason: "Explicit new submission may duplicate delivery".into(),
                },
            )),
        };
        assert_eq!(
            h.operations()
                .resolve_operation(decision.clone())
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
        if let Some(v2::resolve_operation_request::Decision::Resend(resend)) =
            &mut decision.decision
        {
            resend.accept_duplicate_risk = true;
        }
        let mut foreign = decision.clone();
        foreign.account_id = beta;
        assert_eq!(
            h.operations()
                .resolve_operation(foreign)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::NotFound
        );
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 1);
        let mut first_client = h.operations();
        let mut second_client = h.operations();
        let (first, second) = tokio::join!(
            first_client.resolve_operation(decision.clone()),
            second_client.resolve_operation(decision.clone())
        );
        let first = first?.into_inner();
        let second = second?.into_inner();
        assert_eq!(first, second);
        assert_eq!(first.state, "uncertain");
        assert_eq!(first.disposition.as_deref(), Some("resend_requested"));
        let replacement = first.resolutions[0].replacement_id.clone().unwrap();
        assert_ne!(replacement, original.id);
        applied(&h, &account, &replacement).await?;
        h.shutdown().await?;
        h.restart().await?;
        assert_eq!(
            h.operations()
                .resolve_operation(decision)
                .await?
                .into_inner(),
            first
        );
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(snapshot["requests"]["smtp DATA"], 2);
        assert_eq!(snapshot["accepted"]["smtp DATA"], 2);
        assert_eq!(snapshot["smtp_deliveries"].as_array().unwrap().len(), 2);
        for envelope in snapshot["smtp_deliveries"].as_array().unwrap() {
            assert_eq!(envelope["sender"], "alpha@example.test");
            assert_eq!(
                envelope["recipients"],
                json!(["beta@example.test", "blind@example.test"])
            );
        }
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 2);
        assert_eq!(
            snapshot["requests"]["imap APPEND"].as_u64().unwrap_or(0),
            u64::from(!server_sent)
        );
        assert_eq!(
            snapshot["accepted"]["imap APPEND"].as_u64().unwrap_or(0),
            u64::from(!server_sent)
        );
        let sent = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
            .await?;
        assert_eq!(
            sent["messages"].as_array().unwrap().len(),
            if server_sent { 2 } else { 1 }
        );
        for id in [original.id, replacement.clone()] {
            let attempts = h
                .operations()
                .list_attempts(v2::ListOperationAttemptsRequest {
                    account_id: account.clone(),
                    operation_id: id.clone(),
                    page_size: 100,
                    page_token: None,
                })
                .await?
                .into_inner();
            let receipts = attempts
                .items
                .iter()
                .flat_map(|a| &a.receipts)
                .collect::<Vec<_>>();
            assert_eq!(
                receipts
                    .iter()
                    .filter(|r| r.kind == "smtp_accepted")
                    .count(),
                usize::from(id == replacement)
            );
            assert_eq!(
                receipts.iter().filter(|r| r.kind == "sent_copy").count(),
                usize::from(id == replacement)
            );
        }
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
