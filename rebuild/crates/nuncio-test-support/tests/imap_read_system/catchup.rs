use super::*;

#[tokio::test]
async fn initial_imap_sync_catches_up_when_mail_arrives_during_download() -> Result<(), TestError> {
    changed_during_download("append", &[]).await
}
#[tokio::test]
async fn initial_imap_sync_refreshes_flags_changed_during_download() -> Result<(), TestError> {
    changed_during_download("flags", &[]).await
}
#[tokio::test]
async fn initial_imap_sync_drops_staged_messages_expunged_during_download() -> Result<(), TestError>
{
    changed_during_download("expunge", &[]).await
}

#[tokio::test]
async fn initial_imap_sync_handles_notifications_without_optional_extensions(
) -> Result<(), TestError> {
    for change in ["append", "flags", "expunge"] {
        changed_during_download(change, &["MOVE", "UIDPLUS", "CONDSTORE", "QRESYNC"]).await?;
    }
    Ok(())
}

async fn changed_during_download(change: &str, hidden: &[&str]) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(hidden).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = h
        .accounts()
        .connect_imap(request(&mock, false, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .unwrap()
        .id;
    let reads_before = mock.control(json!({"command":"snapshot"})).await?["requests"]
        ["imap UID FETCH"]
        .as_u64()
        .unwrap_or(0);
    h.arm("imap-before-message-stage")?;
    let run = h
        .authenticated()
        .start_sync(v2::StartSyncRequest {
            account_id: account.clone(),
            full: true,
            fetch_message_id: None,
        })
        .await?
        .into_inner();
    h.wait("imap-before-message-stage").await?;
    let raw = b"From: sender@example.test\r\nTo: alpha@example.test\r\nSubject: Arrival during download\r\nMessage-ID: <arrival-during-download@example.test>\r\n\r\nNew independent message\r\n";
    match change {
        "append" => {
            mock.control(json!({"command":"append","account":"alpha@example.test","mailbox":"INBOX","raw_base64":STANDARD.encode(raw)})).await?;
        }
        "flags" => {
            mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":["\\Seen","\\Flagged"],"remove":[]})).await?;
        }
        "expunge" => {
            mock.control(json!({"command":"expunge","account":"alpha@example.test","mailbox":"INBOX","uid":1})).await?;
        }
        _ => unreachable!(),
    }
    let before = imap_effects::effects(&mut mock).await?;
    h.release("imap-before-message-stage")?;
    let completed = finished(&h, &account, &run.id).await?;
    assert_eq!(
        completed.state, "succeeded",
        "new arrivals must be caught up in this run: {:?}",
        completed.error_code
    );
    let messages = list(&h, &account).await?;
    let expected_count = match change {
        "append" => 3,
        "flags" => 2,
        "expunge" => 1,
        _ => unreachable!(),
    };
    assert_eq!(messages.items.len(), expected_count);
    assert_eq!(
        completed.processed,
        if change == "append" { 3 } else { 2 },
        "rechecked entries must not inflate progress"
    );
    let reads_after = mock.control(json!({"command":"snapshot"})).await?["requests"]
        ["imap UID FETCH"]
        .as_u64()
        .unwrap_or(0);
    assert_eq!(
        reads_after - reads_before,
        if change == "append" { 5 } else { 4 },
        "catch-up must reuse immutable bodies, not download them twice"
    );
    if change == "append" {
        assert!(messages
            .items
            .iter()
            .any(|m| m.subject.as_deref() == Some("Arrival during download")));
    } else if change == "flags" {
        let item = messages
            .items
            .iter()
            .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
            .unwrap();
        let stored = h
            .mail()
            .get_message(v2::MessageRequest {
                account_id: account.clone(),
                message_id: item.id.clone(),
            })
            .await?
            .into_inner();
        let state: serde_json::Value = serde_json::from_str(&stored.provider_json)?;
        assert!(state["flags"].to_string().contains("Seen"));
        assert!(state["flags"].to_string().contains("Flagged"));
    } else {
        assert_eq!(
            messages.items[0].subject.as_deref(),
            Some("Independent MailPlus fixture 1")
        );
    }
    assert!(messages.coverage.unwrap().cursor.is_some());
    assert_eq!(
        imap_effects::effects(&mut mock).await?,
        before,
        "read recovery cannot cause remote writes or alter another account"
    );
    h.shutdown().await?;
    h.restart().await?;
    assert_eq!(list(&h, &account).await?.items.len(), expected_count);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

async fn finished(h: &SystemHarness, account: &str, id: &str) -> Result<v2::SyncRun, TestError> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let state = h
                .authenticated()
                .get_sync_run(v2::SyncRunRequest {
                    account_id: account.into(),
                    run_id: id.into(),
                })
                .await?
                .into_inner();
            if !matches!(state.state.as_str(), "queued" | "running") {
                break Ok::<_, TestError>(state);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await?
}

#[tokio::test]
async fn continuous_imap_arrivals_stop_at_bound_and_preserve_last_published_snapshot(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = h
        .accounts()
        .connect_imap(request(&mock, false, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .unwrap()
        .id;
    sync(&h, &mut mock, &account, true).await?;
    let old = list(&h, &account).await?;
    h.arm("imap-before-message-stage")?;
    let run = h
        .authenticated()
        .start_sync(v2::StartSyncRequest {
            account_id: account.clone(),
            full: true,
            fetch_message_id: None,
        })
        .await?
        .into_inner();
    for pass in 1..=3 {
        h.wait("imap-before-message-stage").await?;
        let raw=format!("From: sender@example.test\r\nTo: alpha@example.test\r\nSubject: Arrival pass {pass}\r\n\r\nBody {pass}\r\n");
        mock.control(json!({"command":"append","account":"alpha@example.test","mailbox":"INBOX","raw_base64":STANDARD.encode(raw)})).await?;
        let fault = format!("catchup-pass-{pass}");
        if pass < 3 {
            mock.control(json!({"command":"inject","name":fault,"protocol":"imap","verb":"UID SEARCH","phase":"before","action":"withhold"})).await?;
        }
        h.release("imap-before-message-stage")?;
        if pass < 3 {
            mock.control(json!({"command":"wait_fault","name":fault}))
                .await?;
            h.arm("imap-before-message-stage")?;
            mock.control(json!({"command":"release","name":fault}))
                .await?;
        }
    }
    let effects = imap_effects::effects(&mut mock).await?;
    let done = finished(&h, &account, &run.id).await?;
    assert_eq!(done.state, "failed");
    assert_eq!(done.error_code.as_deref(), Some("mailbox_changed"));
    assert_eq!(done.processed, 4);
    assert_eq!(list(&h, &account).await?.items, old.items);
    h.shutdown().await?;
    h.restart().await?;
    let recovered = list(&h, &account).await?;
    assert_eq!(recovered.items, old.items);
    assert_eq!(
        recovered.coverage.unwrap().cursor,
        old.coverage.unwrap().cursor
    );
    sync(&h, &mut mock, &account, true).await?;
    assert_eq!(list(&h, &account).await?.items.len(), 5);
    assert_eq!(imap_effects::effects(&mut mock).await?, effects);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn imap_identity_change_during_download_never_reuses_old_epoch_data() -> Result<(), TestError>
{
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = h
        .accounts()
        .connect_imap(request(&mock, false, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .unwrap()
        .id;
    sync(&h, &mut mock, &account, true).await?;
    let old = list(&h, &account).await?;
    h.arm("imap-before-message-stage")?;
    let run = h
        .authenticated()
        .start_sync(v2::StartSyncRequest {
            account_id: account.clone(),
            full: true,
            fetch_message_id: None,
        })
        .await?
        .into_inner();
    h.wait("imap-before-message-stage").await?;
    mock.control(json!({"command":"uidvalidity","account":"alpha@example.test","mailbox":"INBOX","value":9517})).await?;
    let remote = imap_effects::effects(&mut mock).await?;
    h.release("imap-before-message-stage")?;
    let done = finished(&h, &account, &run.id).await?;
    assert_eq!(done.state, "failed");
    assert_eq!(list(&h, &account).await?.items, old.items);
    h.shutdown().await?;
    h.restart().await?;
    sync(&h, &mut mock, &account, true).await?;
    let after = list(&h, &account).await?;
    assert_eq!(after.items.len(), 2);
    for item in after.items {
        assert!(old.items.iter().all(|old| old.id != item.id));
        let message = h
            .mail()
            .get_message(v2::MessageRequest {
                account_id: account.clone(),
                message_id: item.id,
            })
            .await?
            .into_inner();
        let state: serde_json::Value = serde_json::from_str(&message.provider_json)?;
        assert_eq!(state["placement"]["uid_validity"], 9517);
    }
    assert_eq!(imap_effects::effects(&mut mock).await?, remote);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
