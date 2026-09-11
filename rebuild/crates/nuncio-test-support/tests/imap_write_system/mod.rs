mod recovery;
mod recovery_sent;
mod server_sent;
mod smtp;
mod smtp_confirmation;
mod smtp_profiles;
mod smtp_resend;
mod transfers;
mod trash;
use super::imap_read_system::{list, sync};
use super::*;
async fn applied(h: &SystemHarness, account: &str, id: &str) -> Result<v2::Operation, TestError> {
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
        if op.state == "applied" {
            return Ok(op);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "operation state {} error {:?}",
            op.state,
            op.error_code
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
#[tokio::test]
async fn imap_flag_intents_preserve_other_flags_and_reconcile_lost_acknowledgement_without_repeating_store(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&["CONDSTORE", "QRESYNC"]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = h
        .accounts()
        .connect_imap(request(&mock, true, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .ok_or("account missing")?
        .id;
    sync(&h, &mut mock, &account, true).await?;
    let messages = list(&h, &account).await?;
    let message = messages
        .items
        .iter()
        .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
        .ok_or("message missing")?
        .id
        .clone();
    let caps = h
        .mail()
        .get_capabilities(v2::MailCapabilitiesRequest {
            account_id: account.clone(),
        })
        .await?
        .into_inner();
    assert!(caps.read_unread && caps.star_unstar);
    assert!(!caps.existing_labels);
    mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":["\\Answered"],"remove":[]})).await?;
    for (ordinal, action, flag, present) in [
        (
            0,
            v2::change_message_request::Action::Read(v2::MailReadChange { read: true }),
            "\\Seen",
            true,
        ),
        (
            1,
            v2::change_message_request::Action::Star(v2::MailStarChange { starred: true }),
            "\\Flagged",
            true,
        ),
        (
            2,
            v2::change_message_request::Action::Read(v2::MailReadChange { read: false }),
            "\\Seen",
            false,
        ),
        (
            3,
            v2::change_message_request::Action::Star(v2::MailStarChange { starred: false }),
            "\\Flagged",
            false,
        ),
    ] {
        let name = format!("lost-store-{ordinal}");
        mock.control(json!({"command":"inject","name":name,"protocol":"imap","verb":"UID STORE","phase":"after","action":"disconnect"})).await?;
        let input = v2::ChangeMessageRequest {
            account_id: account.clone(),
            message_id: message.clone(),
            request_id: format!("a4fcb643-d89d-43a9-812c-{ordinal:012}"),
            action: Some(action),
        };
        let queued = h.mail().change_message(input.clone()).await?.into_inner();
        let done = applied(&h, &account, &queued.id).await?;
        assert!(!done.needs_reconciliation);
        assert_eq!(
            h.mail().change_message(input).await?.into_inner().id,
            done.id
        );
        assert_eq!(
            mock.control(json!({"command":"wait_fault","name":name}))
                .await?["entered"],
            true
        );
        let remote = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        let flags = remote["messages"][0]["flags"]
            .as_array()
            .ok_or("flags missing")?;
        assert_eq!(flags.contains(&json!(flag)), present);
        assert!(flags.contains(&json!("\\Answered")));
        let detail = h
            .mail()
            .get_message(v2::MessageRequest {
                account_id: account.clone(),
                message_id: message.clone(),
            })
            .await?
            .into_inner();
        let state: serde_json::Value = serde_json::from_str(&detail.provider_json)?;
        let local = state["flags"].as_array().unwrap();
        assert_eq!(local.contains(&json!(flag)), present);
        assert!(local.contains(&json!("\\Answered")));
        let observation = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(observation["requests"]["imap UID STORE"], ordinal + 1);
        assert_eq!(observation["accepted"]["imap UID STORE"], ordinal + 1);
        let attempts = h
            .operations()
            .list_attempts(v2::ListOperationAttemptsRequest {
                account_id: account.clone(),
                operation_id: done.id,
                page_size: 100,
                page_token: None,
            })
            .await?
            .into_inner();
        assert_eq!(attempts.items.len(), 2);
        assert_eq!(
            attempts
                .items
                .iter()
                .find(|a| a.ordinal == 1)
                .unwrap()
                .outcome
                .as_deref(),
            Some("uncertain")
        );
        assert_eq!(
            attempts
                .items
                .iter()
                .find(|a| a.ordinal == 2)
                .unwrap()
                .receipts[0]
                .source,
            "positive_read"
        );
    }
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn unavailable_flag_reconciliation_retains_uncertainty_and_epoch_changes_never_write_reused_uids(
) -> Result<(), TestError> {
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
    let beta = h
        .accounts()
        .connect_imap(request(&mock, false, "beta@example.test").await?)
        .await?
        .into_inner()
        .account
        .unwrap()
        .id;
    sync(&h, &mut mock, &account, true).await?;
    sync(&h, &mut mock, &beta, true).await?;
    let messages = list(&h, &account).await?;
    let id = messages
        .items
        .iter()
        .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
        .unwrap()
        .id
        .clone();
    let input = v2::ChangeMessageRequest {
        account_id: account.clone(),
        message_id: id.clone(),
        request_id: "5f922839-d651-45ec-90f1-491da7241249".into(),
        action: Some(v2::change_message_request::Action::Read(
            v2::MailReadChange { read: true },
        )),
    };
    let mut foreign = input.clone();
    foreign.account_id = beta;
    assert_eq!(
        h.mail().change_message(foreign).await.unwrap_err().code(),
        tonic::Code::NotFound
    );
    mock.control(json!({"command":"inject","name":"flags-lost","protocol":"imap","verb":"UID STORE","phase":"after","action":"disconnect"})).await?;
    mock.control(json!({"command":"inject","name":"flags-read-unavailable","protocol":"imap","verb":"EXAMINE","phase":"before","action":"reject"})).await?;
    let op = h.mail().change_message(input.clone()).await?.into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let current = h
            .operations()
            .get_operation(v2::OperationRequest {
                account_id: account.clone(),
                operation_id: op.id.clone(),
            })
            .await?
            .into_inner();
        if current.error_code.as_deref() == Some("imap_preflight_unavailable") {
            assert_eq!(current.state, "uncertain");
            assert!(current.needs_reconciliation);
            assert!(current.next_attempt_at_ms.is_some());
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "state {} error {:?}",
            current.state,
            current.error_code
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let remote = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
        .await?;
    assert!(remote["messages"][0]["flags"]
        .as_array()
        .unwrap()
        .contains(&json!("\\Seen")));
    let detail = h
        .mail()
        .get_message(v2::MessageRequest {
            account_id: account.clone(),
            message_id: id.clone(),
        })
        .await?
        .into_inner();
    let local: serde_json::Value = serde_json::from_str(&detail.provider_json)?;
    assert!(!local["flags"]
        .as_array()
        .unwrap()
        .contains(&json!("\\Seen")));
    assert_eq!(
        mock.control(json!({"command":"snapshot"})).await?["requests"]["imap UID STORE"],
        1
    );
    let done = applied(&h, &account, &op.id).await?;
    let attempts = h
        .operations()
        .list_attempts(v2::ListOperationAttemptsRequest {
            account_id: account.clone(),
            operation_id: done.id.clone(),
            page_size: 100,
            page_token: None,
        })
        .await?
        .into_inner();
    assert_eq!(attempts.items.len(), 3);
    let failed_read = attempts.items.iter().find(|a| a.ordinal == 2).unwrap();
    assert_eq!(failed_read.kind, "reconcile");
    assert_eq!(failed_read.outcome.as_deref(), Some("uncertain"));
    assert!(failed_read.receipts.is_empty());
    assert_eq!(
        attempts
            .items
            .iter()
            .find(|a| a.ordinal == 3)
            .unwrap()
            .receipts[0]
            .source,
        "positive_read"
    );
    let mut noop = input.clone();
    noop.request_id = "a89f16d2-ae5b-4e26-bc32-6f1bf366f3f9".into();
    let noop = h.mail().change_message(noop).await?.into_inner();
    applied(&h, &account, &noop.id).await?;
    mock.control(json!({"command":"uidvalidity","account":"alpha@example.test","mailbox":"INBOX","value":9011})).await?;
    let mut stale = input;
    stale.request_id = "c84ca48c-dc0a-48ce-b0d0-27d97adebe8e".into();
    stale.action = Some(v2::change_message_request::Action::Star(
        v2::MailStarChange { starred: true },
    ));
    let stale = h.mail().change_message(stale).await?.into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let current = h
            .operations()
            .get_operation(v2::OperationRequest {
                account_id: account.clone(),
                operation_id: stale.id.clone(),
            })
            .await?
            .into_inner();
        if current.state == "conflict" {
            assert_eq!(
                current.error_code.as_deref(),
                Some("imap_uid_validity_changed")
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "state {}",
            current.state
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let observation = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(observation["requests"]["imap UID STORE"], 1);
    assert_eq!(observation["accepted"]["imap UID STORE"], 1);
    let remote = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
        .await?;
    assert!(!remote["messages"][0]["flags"]
        .as_array()
        .unwrap()
        .contains(&json!("\\Flagged")));
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn archive_uses_native_move_or_scoped_copy_expunge_and_preserves_unrelated_deleted_message(
) -> Result<(), TestError> {
    for hidden in [vec![], vec!["MOVE"]] {
        let native = hidden.is_empty();
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
        let items = list(&h, &account).await?;
        let source = items
            .items
            .iter()
            .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
            .unwrap();
        let raw = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}),
            )
            .await?;
        let intent = v2::ChangeMessageRequest {
            account_id: account.clone(),
            message_id: source.id.clone(),
            request_id: "76dcd1a0-2e82-457a-8d79-d511b22680da".into(),
            action: Some(v2::change_message_request::Action::Archive(
                v2::MailArchiveChange {},
            )),
        };
        let queued = h.mail().change_message(intent.clone()).await?.into_inner();
        let done = applied(&h, &account, &queued.id).await?;
        assert_eq!(
            h.mail().change_message(intent).await?.into_inner().id,
            done.id
        );
        let inbox = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        assert_eq!(inbox["messages"].as_array().unwrap().len(), 1);
        assert_eq!(inbox["messages"][0]["uid"], 2);
        assert!(inbox["messages"][0]["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("\\Deleted")));
        let archive = mock
            .control(
                json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Archive"}),
            )
            .await?;
        assert_eq!(archive["messages"].as_array().unwrap().len(), 1);
        let uid = archive["messages"][0]["uid"].as_u64().unwrap();
        let copied=mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":"Archive","uid":uid})).await?;
        assert_eq!(copied["raw_base64"], raw["raw_base64"]);
        let observation = mock.control(json!({"command":"snapshot"})).await?;
        let count = |verb: &str| {
            observation["requests"][format!("imap {verb}")]
                .as_u64()
                .unwrap_or(0)
        };
        assert_eq!(count("UID MOVE"), u64::from(native));
        assert_eq!(count("UID COPY"), u64::from(!native));
        assert_eq!(count("UID EXPUNGE"), u64::from(!native));
        assert_eq!(count("EXPUNGE"), 0);
        let local = list(&h, &account).await?;
        assert_eq!(local.items.len(), 2);
        let moved = local
            .items
            .iter()
            .find(|m| m.subject == source.subject)
            .unwrap();
        assert_ne!(moved.id, source.id);
        assert_eq!(moved.collections.len(), 1);
        assert_eq!(moved.collections[0].name.as_deref(), Some("Archive"));
        let output = h
            .mail()
            .get_message(v2::MessageRequest {
                account_id: account.clone(),
                message_id: moved.id.clone(),
            })
            .await?
            .into_inner();
        let placement: serde_json::Value = serde_json::from_str(&output.provider_json)?;
        assert_eq!(placement["placement"]["uid"], uid);
        let noop = h
            .mail()
            .change_message(v2::ChangeMessageRequest {
                account_id: account.clone(),
                message_id: moved.id.clone(),
                request_id: "e9b9a4ad-184f-413b-b21c-d3f1fe550f60".into(),
                action: Some(v2::change_message_request::Action::Archive(
                    v2::MailArchiveChange {},
                )),
            })
            .await?
            .into_inner();
        applied(&h, &account, &noop.id).await?;
        let again = h
            .mail()
            .get_message(v2::MessageRequest {
                account_id: account.clone(),
                message_id: moved.id.clone(),
            })
            .await?
            .into_inner();
        assert_eq!(again.attachments, output.attachments);
        let after_noop = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(
            after_noop["requests"].get("imap UID MOVE"),
            observation["requests"].get("imap UID MOVE")
        );
        assert_eq!(
            after_noop["requests"].get("imap UID COPY"),
            observation["requests"].get("imap UID COPY")
        );
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
