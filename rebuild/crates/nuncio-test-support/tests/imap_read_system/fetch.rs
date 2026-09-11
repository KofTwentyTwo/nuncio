use super::*;
async fn fetch(h: &SystemHarness, account: &str, message: &str) -> Result<v2::SyncRun, TestError> {
    let mut run = h
        .authenticated()
        .start_sync(v2::StartSyncRequest {
            account_id: account.into(),
            full: false,
            fetch_message_id: Some(message.into()),
        })
        .await?
        .into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(run.state.as_str(), "queued" | "running") {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        run = h
            .authenticated()
            .get_sync_run(v2::SyncRunRequest {
                account_id: account.into(),
                run_id: run.id,
            })
            .await?
            .into_inner();
    }
    assert_eq!(run.mode, "fetch");
    Ok(run)
}
#[tokio::test]
async fn explicit_imap_fetch_reloads_one_placement_without_advancing_global_coverage(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = h
        .accounts()
        .connect_imap(request(&mock, true, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .ok_or("account missing")?
        .id;
    mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"INBOX","uid":1,"destination":"Archive"})).await?;
    sync(&h, &mut mock, &account, true).await?;
    let before = list(&h, &account).await?;
    let message = before
        .items
        .iter()
        .find(|m| {
            m.collections[0].name.as_deref() == Some("INBOX")
                && m.subject.as_deref() == Some("Independent MailPlus fixture 0")
        })
        .ok_or("target missing")?
        .id
        .clone();
    mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":["\\Flagged"],"remove":[]})).await?;
    mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"INBOX","uid":2,"destination":"INBOX"})).await?;
    let fetched = fetch(&h, &account, &message).await?;
    assert_eq!(fetched.state, "succeeded", "{:?}", fetched.error_code);
    let after = list(&h, &account).await?;
    assert_eq!(after.items, before.items);
    assert_eq!(after.coverage, before.coverage);
    let detail = h
        .mail()
        .get_message(v2::MessageRequest {
            account_id: account.clone(),
            message_id: message.clone(),
        })
        .await?
        .into_inner();
    let state: serde_json::Value = serde_json::from_str(&detail.provider_json)?;
    assert!(state["flags"]
        .as_array()
        .unwrap()
        .contains(&json!("\\Flagged")));
    mock.control(json!({"command":"uidvalidity","account":"alpha@example.test","mailbox":"INBOX","value":9991})).await?;
    let stale = fetch(&h, &account, &message).await?;
    assert_eq!(stale.state, "failed");
    assert_eq!(stale.error_code.as_deref(), Some("not_found"));
    assert_eq!(list(&h, &account).await?.items, after.items);
    assert_eq!(list(&h, &account).await?.coverage, before.coverage);
    sync(&h, &mut mock, &account, false).await?;
    let refreshed = list(&h, &account).await?;
    assert_eq!(refreshed.items.len(), 4);
    assert!(refreshed.items.iter().all(|m| m.id != message));
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
