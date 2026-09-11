use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[tokio::test]
async fn server_sent_requires_unique_current_placement_and_matching_content(
) -> Result<(), TestError> {
    for scenario in [
        "missing",
        "changed_content",
        "duplicate",
        "epoch",
        "recreated_folder",
        "missing_folder",
    ] {
        let mut mock = MockMailPlus::start_with_sent_policy(&[], true).await?;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
        let mut connect = request(&mock, true, "alpha@example.test").await?;
        connect.config.as_mut().unwrap().sent_policy = v2::SentPolicy::Server.into();
        if scenario == "missing_folder" {
            mock.control(json!({"command":"folder_create","account":"alpha@example.test","mailbox":"Custom Sent"})).await?;
            connect.config.as_mut().unwrap().sent_folder = "Custom Sent".into();
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
        let before = list(&h, &account).await?;
        let draft = h.mail().save_draft(v2::SaveDraftRequest {
            account_id: account.clone(), draft_id: None, expected_version: None,
            content: Some(serde_json::from_value(json!({"to":[{"address":"beta@example.test"}],"subject":"Server Sent proof","text":"Original body"}))?),
        }).await?.into_inner();
        let input = v2::SendDraftRequest {
            account_id: account.clone(),
            draft_id: draft.id,
            request_id: "98192262-77e9-4199-ae49-ea7047953478".into(),
            expected_version: Some(draft.version),
        };
        h.arm("operation_after_smtp_acceptance")?;
        let op = h.mail().send_draft(input.clone()).await?.into_inner();
        h.wait("operation_after_smtp_acceptance").await?;
        let saved = mock
            .control(json!({"command":"wait_server_sent","count":1}))
            .await;
        if let Err(error) = saved {
            eprintln!(
                "{scenario}: independent AutoSent failed; artifacts {}",
                mock.artifacts.display()
            );
            h.release("operation_after_smtp_acceptance")?;
            h.shutdown().await?;
            let _ = mock.shutdown().await;
            return Err(error);
        }
        assert_eq!(saved?["completed"], 1);
        let sent = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
            .await?;
        assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
        let uid = sent["messages"][0]["uid"].as_u64().unwrap();
        let raw = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":uid}),
            )
            .await?["raw_base64"]
            .as_str()
            .unwrap()
            .to_owned();
        let retryable = matches!(scenario, "missing" | "changed_content");
        match scenario {
            "missing" | "changed_content" => {
                mock.control(json!({"command":"expunge","account":"alpha@example.test","mailbox":"Sent","uid":uid})).await?;
                if scenario == "changed_content" {
                    let mut changed = STANDARD.decode(&raw)?;
                    // Same Message-ID and headers; a different body is not proof.
                    changed.extend_from_slice(b"Other content\r\n");
                    mock.control(json!({"command":"append","account":"alpha@example.test","mailbox":"Sent","raw_base64":STANDARD.encode(changed)})).await?;
                }
            }
            "duplicate" => {
                mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"Sent","uid":uid,"destination":"Sent"})).await?;
            }
            "epoch" => {
                mock.control(json!({"command":"uidvalidity","account":"alpha@example.test","mailbox":"Sent","value":sent["uidvalidity"].as_u64().unwrap()+1})).await?;
            }
            "recreated_folder" => {
                mock.control(json!({"command":"folder_rename","account":"alpha@example.test","mailbox":"Sent","destination":"Previous Sent"})).await?;
                let replacement=mock.control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"})).await?;
                assert!(replacement["messages"].as_array().unwrap().is_empty());
                assert_ne!(replacement["uidvalidity"], sent["uidvalidity"]);
            }
            "missing_folder" => {
                mock.control(json!({"command":"folder_rename","account":"alpha@example.test","mailbox":"Custom Sent","destination":"Previous Custom Sent"})).await?;
            }
            _ => unreachable!(),
        }
        h.release("operation_after_smtp_acceptance")?;
        let expected_code = match scenario {
            "duplicate" => "smtp_server_sent_ambiguous",
            "epoch" | "recreated_folder" => "imap_sent_epoch_changed",
            "missing_folder" => "imap_folder_missing",
            _ => "smtp_server_sent_unconfirmed",
        };
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
            if current.state == "uncertain"
                && current.error_code.as_deref() == Some(expected_code)
                && current.needs_reconciliation == retryable
                && current.next_attempt_at_ms.is_some() == retryable
            {
                assert_eq!(current.needs_reconciliation, retryable);
                assert_eq!(current.next_attempt_at_ms.is_some(), retryable);
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{scenario}: {} {:?}",
                current.state,
                current.error_code
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(list(&h, &account).await?.items, before.items);
        let attempts = h
            .operations()
            .list_attempts(v2::ListOperationAttemptsRequest {
                account_id: account.clone(),
                operation_id: op.id.clone(),
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
        assert!(!receipts
            .iter()
            .any(|r| matches!(r.kind.as_str(), "server_sent_observed" | "sent_copy")));
        if retryable {
            if scenario == "changed_content" {
                let altered=mock.control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"})).await?;
                assert_eq!(altered["messages"].as_array().unwrap().len(), 1);
                mock.control(json!({"command":"expunge","account":"alpha@example.test","mailbox":"Sent","uid":altered["messages"][0]["uid"]})).await?;
            }
            mock.control(json!({"command":"append","account":"alpha@example.test","mailbox":"Sent","raw_base64":raw})).await?;
            applied(&h, &account, &op.id).await?;
        }
        h.shutdown().await?;
        h.restart().await?;
        assert_eq!(h.mail().send_draft(input).await?.into_inner().id, op.id);
        let current = h
            .operations()
            .get_operation(v2::OperationRequest {
                account_id: account.clone(),
                operation_id: op.id.clone(),
            })
            .await?
            .into_inner();
        assert_eq!(
            current.state,
            if retryable { "applied" } else { "uncertain" }
        );
        assert!(!current.needs_reconciliation);
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(snapshot["requests"]["smtp DATA"], 1);
        assert_eq!(snapshot["accepted"]["smtp DATA"], 1);
        assert_eq!(snapshot["smtp_deliveries"].as_array().unwrap().len(), 1);
        assert_eq!(
            snapshot["smtp_deliveries"][0]["recipients"],
            json!(["beta@example.test"])
        );
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 1);
        assert_eq!(snapshot["requests"]["imap APPEND"].as_u64().unwrap_or(0), 0);
        assert_eq!(snapshot["accepted"]["imap APPEND"].as_u64().unwrap_or(0), 0);
        let folder = if scenario == "recreated_folder" {
            "Previous Sent"
        } else {
            "Sent"
        };
        let remote = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":folder}))
            .await?;
        assert_eq!(
            remote["messages"].as_array().unwrap().len(),
            if scenario == "duplicate" { 2 } else { 1 }
        );
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
