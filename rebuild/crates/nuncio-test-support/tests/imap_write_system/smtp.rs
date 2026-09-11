use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn smtp_and_append_lost_acknowledgements_do_not_trigger_duplicate_remote_effects(
) -> Result<(), TestError> {
    for (protocol, verb, phase, action, expected, sent_copies, data_requests) in [
        ("smtp", "DATA", "after", "disconnect", "uncertain", 0, 1),
        ("imap", "APPEND", "after", "disconnect", "uncertain", 1, 1),
        ("smtp", "DATA", "before", "reject", "applied", 1, 2),
        ("imap", "APPEND", "before", "reject", "applied", 1, 1),
    ] {
        let mut mock = MockMailPlus::start(&[]).await?;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
        let account = h
            .accounts()
            .connect_imap(request(&mock, true, "alpha@example.test").await?)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id;
        sync(&h, &mut mock, &account, true).await?;
        let draft=h.mail().save_draft(v2::SaveDraftRequest{account_id:account.clone(),draft_id:None,expected_version:None,content:Some(serde_json::from_value(json!({"to":[{"address":"beta@example.test"}],"subject":"SMTP fault","text":"One intended delivery"}))?)}).await?.into_inner();
        mock.control(json!({"command":"inject","name":"send-fault","protocol":protocol,"verb":verb,"phase":phase,"action":action})).await?;
        let input = v2::SendDraftRequest {
            account_id: account.clone(),
            draft_id: draft.id,
            request_id: "ea85f610-4afc-4696-8cd9-c51773427021".into(),
            expected_version: Some(draft.version),
        };
        let op = h.mail().send_draft(input.clone()).await?.into_inner();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
        loop {
            let current = h
                .operations()
                .get_operation(v2::OperationRequest {
                    account_id: account.clone(),
                    operation_id: op.id.clone(),
                })
                .await?
                .into_inner();
            if current.state == expected && !current.needs_reconciliation {
                if expected == "uncertain" {
                    assert_eq!(
                        current.error_code.as_deref(),
                        Some(if protocol == "smtp" {
                            "smtp_acceptance_unknown"
                        } else {
                            "imap_sent_copy_identity_unknown"
                        })
                    );
                }
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{protocol}/{phase}: {} {:?}",
                current.state,
                current.error_code
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        h.shutdown().await?;
        h.restart().await?;
        assert_eq!(h.mail().send_draft(input).await?.into_inner().id, op.id);
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(snapshot["requests"]["smtp DATA"], data_requests);
        assert_eq!(snapshot["accepted"]["smtp DATA"], 1);
        assert_eq!(snapshot["smtp_deliveries"].as_array().unwrap().len(), 1);
        assert_eq!(
            snapshot["smtp_deliveries"][0]["recipients"],
            json!(["beta@example.test"])
        );
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 1);
        assert_eq!(
            snapshot["accepted"]["imap APPEND"].as_u64().unwrap_or(0),
            sent_copies
        );
        assert_eq!(
            snapshot["requests"]["imap APPEND"].as_u64().unwrap_or(0),
            sent_copies + u64::from(protocol == "imap" && phase == "before")
        );
        let sent = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
            .await?;
        assert_eq!(
            sent["messages"].as_array().unwrap().len(),
            sent_copies as usize
        );
        let local = list(&h, &account).await?;
        assert_eq!(
            local
                .items
                .iter()
                .filter(|m| m.subject.as_deref() == Some("SMTP fault"))
                .count(),
            usize::from(expected == "applied")
        );
        let attempts = h
            .operations()
            .list_attempts(v2::ListOperationAttemptsRequest {
                account_id: account.clone(),
                operation_id: op.id,
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
            usize::from(phase == "before" || protocol == "imap")
        );
        assert_eq!(
            receipts.iter().filter(|r| r.kind == "sent_copy").count(),
            usize::from(expected == "applied")
        );
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn smtp_submission_and_client_sent_copy_have_independent_remote_evidence(
) -> Result<(), TestError> {
    for start_tls in [true, false] {
        submission(false, start_tls, &[]).await?;
    }
    Ok(())
}
#[tokio::test]
async fn server_managed_sent_is_observed_without_client_append() -> Result<(), TestError> {
    for start_tls in [true, false] {
        for hidden in [&[][..], &["UIDPLUS"][..]] {
            submission(true, start_tls, hidden).await?;
        }
    }
    Ok(())
}
async fn submission(server_sent: bool, start_tls: bool, hidden: &[&str]) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start_with_sent_policy(hidden, server_sent).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let mut connect = request(&mock, start_tls, "alpha@example.test").await?;
    if server_sent {
        connect.config.as_mut().unwrap().sent_policy = v2::SentPolicy::Server.into();
    }
    let account = h
        .accounts()
        .connect_imap(connect)
        .await?
        .into_inner()
        .account
        .unwrap()
        .id;
    sync(&h, &mut mock, &account, true).await?;
    let content = serde_json::from_value(
        json!({"to":[{"address":"beta@example.test"}],"subject":"SMTP evidence","text":"Original body\n.leading dot\n..two dots\nUnicode café\n"}),
    )?;
    let draft = h
        .mail()
        .save_draft(v2::SaveDraftRequest {
            account_id: account.clone(),
            draft_id: None,
            expected_version: None,
            content: Some(content),
        })
        .await?
        .into_inner();
    let request = v2::SendDraftRequest {
        account_id: account.clone(),
        draft_id: draft.id,
        request_id: "a8ad5c3e-aae7-47cb-bff8-ec3f7fb6a2aa".into(),
        expected_version: Some(draft.version),
    };
    let queued = h.mail().send_draft(request.clone()).await?.into_inner();
    if server_sent {
        assert_eq!(
            mock.control(json!({"command":"wait_server_sent","count":1}))
                .await?["completed"],
            1
        );
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(snapshot["accepted"]["smtp DATA"], 1);
        assert_eq!(snapshot["requests"]["imap APPEND"].as_u64().unwrap_or(0), 0);
        let sent = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
            .await?;
        assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
    }
    let done = applied(&h, &account, &queued.id).await?;
    assert_eq!(h.mail().send_draft(request).await?.into_inner().id, done.id);
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["accepted"]["smtp DATA"], 1);
    assert_eq!(snapshot["smtp_deliveries"].as_array().unwrap().len(), 1);
    let delivery = &snapshot["smtp_deliveries"][0];
    assert_eq!(delivery["sender"], "alpha@example.test");
    assert_eq!(delivery["recipients"], json!(["beta@example.test"]));
    let transport = if start_tls { "starttls" } else { "tls" };
    let smtp = mock
        .control(json!({"command":"smtp","transport":transport}))
        .await?;
    assert_eq!(smtp["total"], 1);
    let capture = mock
        .control(json!({"command":"smtp_raw","transport":transport,"id":smtp["messages"][0]["ID"]}))
        .await?;
    let capture = STANDARD.decode(capture["raw_base64"].as_str().unwrap())?;
    let sent = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
        .await?;
    assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
    let raw = mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":sent["messages"][0]["uid"]})).await?;
    let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
    let wire_size = delivery["wire_size"].as_u64().unwrap() as usize;
    let wire = &capture[capture.len() - wire_size..];
    assert_eq!(
        format!("{:x}", Sha256::digest(wire)),
        delivery["wire_sha256"]
    );
    if server_sent {
        assert_eq!(raw, capture);
    } else {
        assert_eq!(
            format!("{:x}", Sha256::digest(&raw)),
            delivery["wire_sha256"]
        );
        assert_eq!(raw.len(), wire_size);
        assert!(capture.ends_with(&raw));
    }
    assert_eq!(
        snapshot["requests"]["imap APPEND"].as_u64().unwrap_or(0),
        u64::from(!server_sent)
    );
    assert_eq!(
        snapshot["accepted"]["imap APPEND"].as_u64().unwrap_or(0),
        u64::from(!server_sent)
    );
    let local = list(&h, &account).await?;
    let sent = local
        .items
        .iter()
        .find(|m| m.subject.as_deref() == Some("SMTP evidence"))
        .unwrap();
    assert_eq!(sent.collections[0].name.as_deref(), Some("Sent"));
    let attempts = h
        .operations()
        .list_attempts(v2::ListOperationAttemptsRequest {
            account_id: account,
            operation_id: done.id,
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
        1
    );
    assert_eq!(receipts.iter().filter(|r| r.kind == "sent_copy").count(), 1);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
