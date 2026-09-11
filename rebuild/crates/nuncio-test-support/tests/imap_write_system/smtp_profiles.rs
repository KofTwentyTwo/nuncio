use super::*;

async fn draft(h: &SystemHarness, account: &str) -> Result<v2::Draft, TestError> {
    Ok(h.mail().save_draft(v2::SaveDraftRequest {account_id:account.into(),content:Some(serde_json::from_value(json!({"to":[{"address":"beta@example.test"}],"subject":"SMTP preflight profile","text":"Preserve captured destination"}))?),..Default::default()}).await?.into_inner())
}
async fn wait_state(
    h: &SystemHarness,
    account: &str,
    id: &str,
    state: &str,
) -> Result<v2::Operation, TestError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
    loop {
        let op = h
            .operations()
            .get_operation(v2::OperationRequest {
                account_id: account.into(),
                operation_id: id.into(),
            })
            .await?
            .into_inner();
        if op.state == state {
            return Ok(op);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {state}, got {} {:?}",
            op.state,
            op.error_code
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
async fn no_delivery(mock: &mut MockMailPlus) -> Result<(), TestError> {
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    for key in ["requests", "accepted"] {
        for command in ["smtp DATA", "imap APPEND"] {
            assert_eq!(snapshot[key][command].as_u64().unwrap_or(0), 0);
        }
    }
    assert_eq!(snapshot["smtp_deliveries"], json!([]));
    for transport in ["tls", "starttls"] {
        assert_eq!(
            mock.control(json!({"command":"smtp","transport":transport}))
                .await?["total"],
            0
        );
    }
    assert_eq!(
        mock.control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
            .await?["messages"],
        json!([])
    );
    Ok(())
}

#[tokio::test]
async fn missing_uidplus_refuses_client_sent_before_smtp_data_and_remains_failed_after_restart(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&["UIDPLUS"]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let connected = h
        .accounts()
        .connect_imap(request(&mock, true, "alpha@example.test").await?)
        .await?
        .into_inner();
    assert!(!connected.capabilities.unwrap().uidplus);
    let account = connected.account.unwrap().id;
    sync(&h, &mut mock, &account, true).await?;
    let draft = draft(&h, &account).await?;
    let send = v2::SendDraftRequest {
        account_id: account.clone(),
        draft_id: draft.id,
        request_id: "f1f27d20-23df-49a3-b926-3a2a84ef17a6".into(),
        expected_version: Some(draft.version),
    };
    let queued = h.mail().send_draft(send.clone()).await?.into_inner();
    let failed = wait_state(&h, &account, &queued.id, "failed").await?;
    assert_eq!(
        failed.error_code.as_deref(),
        Some("imap_sent_uidplus_required")
    );
    assert!(!failed.needs_reconciliation);
    assert!(failed.next_attempt_at_ms.is_none());
    no_delivery(&mut mock).await?;
    h.shutdown().await?;
    h.restart().await?;
    assert_eq!(h.mail().send_draft(send).await?.into_inner(), failed);
    let attempts = h
        .operations()
        .list_attempts(v2::ListOperationAttemptsRequest {
            account_id: account,
            operation_id: queued.id,
            page_size: 100,
            page_token: None,
        })
        .await?
        .into_inner();
    assert_eq!(attempts.items.len(), 1);
    assert_eq!(attempts.items[0].outcome.as_deref(), Some("rejected"));
    assert!(attempts.items[0].receipts.is_empty());
    no_delivery(&mut mock).await?;
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn queued_smtp_intent_cannot_follow_a_changed_endpoint_and_can_resume_its_original_transport(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let mut original = request(&mock, true, "alpha@example.test").await?;
    let account = h
        .accounts()
        .connect_imap(original.clone())
        .await?
        .into_inner()
        .account
        .unwrap()
        .id;
    original.account_id = Some(account.clone());
    sync(&h, &mut mock, &account, true).await?;
    let draft = draft(&h, &account).await?;
    let send = v2::SendDraftRequest {
        account_id: account.clone(),
        draft_id: draft.id,
        request_id: "d5bf2399-d485-46e9-8c37-7319258a7f28".into(),
        expected_version: Some(draft.version),
    };
    h.arm("operation_before_dispatch")?;
    let queued = h.mail().send_draft(send.clone()).await?.into_inner();
    h.wait("operation_before_dispatch").await?;
    let mut changed = original.clone();
    let endpoint = changed.config.as_mut().unwrap().smtp.as_mut().unwrap();
    endpoint.port = u32::from(mock.ready.ports.smtps);
    endpoint.tls = v2::MailTls::Implicit.into();
    assert_eq!(
        h.accounts()
            .connect_imap(changed)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id,
        account
    );
    h.release("operation_before_dispatch")?;
    let stopped = wait_state(&h, &account, &queued.id, "retry_wait").await?;
    assert_eq!(stopped.error_code.as_deref(), Some("identity_mismatch"));
    no_delivery(&mut mock).await?;
    h.shutdown().await?;
    h.restart().await?;
    no_delivery(&mut mock).await?;
    assert_eq!(h.mail().send_draft(send).await?.into_inner().id, queued.id);
    h.accounts().connect_imap(original).await?;
    applied(&h, &account, &queued.id).await?;
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["requests"]["smtp DATA"], 1);
    assert_eq!(snapshot["accepted"]["smtp DATA"], 1);
    assert_eq!(snapshot["accepted"]["imap APPEND"], 1);
    assert_eq!(
        mock.control(json!({"command":"smtp","transport":"starttls"}))
            .await?["total"],
        1
    );
    assert_eq!(
        mock.control(json!({"command":"smtp","transport":"tls"}))
            .await?["total"],
        0
    );
    let attempts = h
        .operations()
        .list_attempts(v2::ListOperationAttemptsRequest {
            account_id: account,
            operation_id: queued.id,
            page_size: 100,
            page_token: None,
        })
        .await?
        .into_inner();
    let first = attempts.items.iter().find(|a| a.ordinal == 1).unwrap();
    assert_eq!(first.outcome.as_deref(), Some("rejected"));
    assert_eq!(first.error_code.as_deref(), Some("identity_mismatch"));
    assert!(first.receipts.is_empty());
    assert_eq!(
        attempts
            .items
            .iter()
            .filter(|a| a.outcome.as_deref() == Some("applied"))
            .count(),
        1
    );
    assert_eq!(
        attempts
            .items
            .iter()
            .flat_map(|a| &a.receipts)
            .filter(|r| r.kind == "smtp_accepted")
            .count(),
        1
    );
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
