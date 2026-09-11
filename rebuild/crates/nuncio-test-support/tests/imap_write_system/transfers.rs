use super::*;

#[tokio::test]
async fn transfer_lost_ack_uses_only_known_copyuid_and_unsafe_fallback_never_copies(
) -> Result<(), TestError> {
    for (hidden, verb, expected) in [
        (vec![], Some("UID MOVE"), "applied"),
        (vec!["MOVE"], Some("UID COPY"), "uncertain"),
        (vec!["MOVE", "UIDPLUS"], None, "failed"),
    ] {
        let mut mock = MockMailPlus::start(&hidden).await?;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
        let account = h
            .accounts()
            .connect_imap(request(&mock, true, "alpha@example.test").await?)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id;
        mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":2,"add":["\\Deleted"],"remove":[]})).await?;
        sync(&h, &mut mock, &account, true).await?;
        let before = list(&h, &account).await?;
        let message = before
            .items
            .iter()
            .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
            .unwrap()
            .id
            .clone();
        let raw = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}),
            )
            .await?;
        if let Some(verb) = verb {
            mock.control(json!({"command":"inject","name":"transfer-lost","protocol":"imap","verb":verb,"phase":"after","action":"disconnect"})).await?;
        }
        let input = v2::ChangeMessageRequest {
            account_id: account.clone(),
            message_id: message,
            request_id: "c340fefa-e542-4d95-833f-4b2e92e10d5a".into(),
            action: Some(v2::change_message_request::Action::Archive(
                v2::MailArchiveChange {},
            )),
        };
        let op = h.mail().change_message(input.clone()).await?.into_inner();
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
                        Some("imap_copy_identity_unknown")
                    );
                }
                if expected == "failed" {
                    assert_eq!(
                        current.error_code.as_deref(),
                        Some("imap_safe_move_unsupported")
                    );
                }
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "expected {expected}, got {} {:?}",
                current.state,
                current.error_code
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        h.shutdown().await?;
        h.restart().await?;
        assert_eq!(h.mail().change_message(input).await?.into_inner().id, op.id);
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(
            snapshot["requests"]["imap UID MOVE"].as_u64().unwrap_or(0),
            u64::from(verb == Some("UID MOVE"))
        );
        assert_eq!(
            snapshot["requests"]["imap UID COPY"].as_u64().unwrap_or(0),
            u64::from(verb == Some("UID COPY"))
        );
        assert_eq!(
            snapshot["requests"]["imap EXPUNGE"].as_u64().unwrap_or(0),
            0
        );
        assert_eq!(
            snapshot["requests"]["imap UID EXPUNGE"]
                .as_u64()
                .unwrap_or(0),
            0
        );
        let inbox = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        assert_eq!(
            inbox["messages"].as_array().unwrap().len(),
            if expected == "applied" { 1 } else { 2 }
        );
        assert!(inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["uid"] == 2));
        let target = mock
            .control(
                json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Archive"}),
            )
            .await?;
        assert_eq!(
            target["messages"].as_array().unwrap().len(),
            usize::from(verb.is_some())
        );
        if verb.is_some() {
            let copied = mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":"Archive","uid":1})).await?;
            assert_eq!(copied["raw_base64"], raw["raw_base64"]);
        }
        let local = list(&h, &account).await?;
        if expected == "applied" {
            let copy = local
                .items
                .iter()
                .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
                .unwrap();
            assert_eq!(copy.collections[0].name.as_deref(), Some("Archive"));
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
            let dispatch = attempts.items.iter().find(|a| a.ordinal == 1).unwrap();
            assert_eq!(dispatch.outcome.as_deref(), Some("uncertain"));
            assert_eq!(dispatch.receipts.len(), 1);
            assert_eq!(dispatch.receipts[0].kind, "imap_copy");
            let recovery = attempts.items.iter().find(|a| a.ordinal == 2).unwrap();
            assert_eq!(recovery.receipts[0].source, "positive_read");
        } else {
            assert_eq!(local.items, before.items);
        }
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn folder_actions_scope_destination_to_account_and_conflict_on_a_missing_remote_folder(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let mut accounts = Vec::new();
    let mut targets = Vec::new();
    for user in ["alpha@example.test", "beta@example.test"] {
        let account = h
            .accounts()
            .connect_imap(request(&mock, true, user).await?)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id;
        sync(&h, &mut mock, &account, true).await?;
        let folders = h
            .mail()
            .list_collections(v2::ListCollectionsRequest {
                account_id: account.clone(),
            })
            .await?
            .into_inner();
        targets.push(
            folders
                .items
                .into_iter()
                .find(|f| f.name.as_deref() == Some("Archive"))
                .unwrap()
                .id,
        );
        accounts.push(account);
    }
    let before = list(&h, &accounts[0]).await?;
    let message = before
        .items
        .iter()
        .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
        .unwrap()
        .id
        .clone();
    let caps = h
        .mail()
        .get_capabilities(v2::MailCapabilitiesRequest {
            account_id: accounts[0].clone(),
        })
        .await?
        .into_inner();
    assert!(caps.move_copy);
    for (index, destination, code) in [
        (0, targets[1].clone(), tonic::Code::NotFound),
        (1, String::new(), tonic::Code::InvalidArgument),
        (
            2,
            "af058826-2145-4a44-b4bf-9bc6c9e3877e".into(),
            tonic::Code::NotFound,
        ),
    ] {
        for (offset, copy) in [(0, false), (10, true)] {
            let change = v2::MailFolderChange {
                destination_collection_id: destination.clone(),
            };
            let action = if copy {
                v2::change_message_request::Action::Copy(change)
            } else {
                v2::change_message_request::Action::Move(change)
            };
            let input = v2::ChangeMessageRequest {
                account_id: accounts[0].clone(),
                message_id: message.clone(),
                request_id: format!("8f463c79-d6ac-494c-8668-{:012}", index + offset),
                action: Some(action),
            };
            assert_eq!(
                h.mail().change_message(input).await.unwrap_err().code(),
                code
            );
        }
    }
    // Capture the catalog's original target identity, then remove that remote
    // folder before dispatch. The engine must not reinterpret it as INBOX.
    mock.control(
        json!({"command":"folder_delete","account":"alpha@example.test","mailbox":"Archive"}),
    )
    .await?;
    let op = h
        .mail()
        .change_message(v2::ChangeMessageRequest {
            account_id: accounts[0].clone(),
            message_id: message,
            request_id: "43128c73-e724-4780-9f84-0414d4d12d3a".into(),
            action: Some(v2::change_message_request::Action::Copy(
                v2::MailFolderChange {
                    destination_collection_id: targets[0].clone(),
                },
            )),
        })
        .await?
        .into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let current = h
            .operations()
            .get_operation(v2::OperationRequest {
                account_id: accounts[0].clone(),
                operation_id: op.id.clone(),
            })
            .await?
            .into_inner();
        if current.state == "conflict" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "unexpected {} {:?}",
            current.state,
            current.error_code
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(list(&h, &accounts[0]).await?.items, before.items);
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    for verb in [
        "imap UID MOVE",
        "imap UID COPY",
        "imap UID STORE",
        "imap UID EXPUNGE",
        "imap EXPUNGE",
    ] {
        assert_eq!(snapshot["requests"][verb].as_u64().unwrap_or(0), 0);
    }
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
