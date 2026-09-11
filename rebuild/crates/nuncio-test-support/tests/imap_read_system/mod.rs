mod fetch;
mod repair;
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};

async fn run(h: &SystemHarness, account: &str, full: bool) -> Result<v2::SyncRun, TestError> {
    let mut run = h
        .authenticated()
        .start_sync(v2::StartSyncRequest {
            account_id: account.into(),
            full,
            fetch_message_id: None,
        })
        .await?
        .into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while matches!(run.state.as_str(), "queued" | "running") {
        if tokio::time::Instant::now() > deadline {
            return Err("IMAP sync exceeded test deadline".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        run = h
            .authenticated()
            .get_sync_run(v2::SyncRunRequest {
                account_id: account.into(),
                run_id: run.id,
            })
            .await?
            .into_inner();
    }
    Ok(run)
}
pub(super) async fn sync(
    h: &SystemHarness,
    mock: &mut MockMailPlus,
    account: &str,
    full: bool,
) -> Result<v2::SyncRun, TestError> {
    let run = run(h, account, full).await?;
    if run.state != "succeeded" {
        eprintln!(
            "Independent read observation: {}",
            mock.control(json!({"command":"snapshot"})).await?
        );
    }
    assert_eq!(run.state, "succeeded", "sync error: {:?}", run.error_code);
    assert_eq!(run.scope, "imap");
    Ok(run)
}
pub(super) async fn list(
    h: &SystemHarness,
    account: &str,
) -> Result<v2::ListMailResponse, TestError> {
    Ok(h.mail()
        .list_messages(v2::ListMailRequest {
            account_id: account.into(),
            page_size: 100,
            ..Default::default()
        })
        .await?
        .into_inner())
}
#[tokio::test]
async fn imap_full_and_delta_preserve_exact_remote_placements_flags_and_uid_epochs(
) -> Result<(), TestError> {
    for hidden in [vec![], vec!["MOVE", "UIDPLUS", "CONDSTORE", "QRESYNC"]] {
        let mut mock = MockMailPlus::start(&hidden).await?;
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
        let first = list(&h, &account).await?;
        assert_eq!(first.items.len(), 3);
        let mut placement_ids = std::collections::BTreeSet::new();
        for item in &first.items {
            assert!(placement_ids.insert(item.provider_id.clone()));
            assert_eq!(item.collections.len(), 1);
            let name = item.collections[0]
                .name
                .as_deref()
                .ok_or("folder missing")?;
            let data = h
                .mail()
                .get_message(v2::MessageRequest {
                    account_id: account.clone(),
                    message_id: item.id.clone(),
                })
                .await?
                .into_inner();
            let state: serde_json::Value = serde_json::from_str(&data.provider_json)?;
            let remote=mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":name,"uid":state["placement"]["uid"]})).await?;
            let expected = STANDARD.decode(remote["raw_base64"].as_str().ok_or("raw missing")?)?;
            let mut stream = h
                .mail()
                .download(v2::DownloadMailRequest {
                    account_id: account.clone(),
                    message_id: item.id.clone(),
                    kind: "raw".into(),
                    attachment_id: None,
                })
                .await?
                .into_inner();
            let mut actual = Vec::new();
            while let Some(chunk) = stream.message().await? {
                assert_eq!(chunk.offset, actual.len() as u64);
                actual.extend(chunk.data);
            }
            assert_eq!(actual, expected);
        }
        sync(&h, &mut mock, &account, false).await?;
        let quiet = list(&h, &account).await?;
        assert_eq!(
            quiet.coverage.as_ref().unwrap().cursor,
            first.coverage.as_ref().unwrap().cursor
        );
        assert_eq!(
            quiet.items.iter().map(|m| &m.id).collect::<Vec<_>>(),
            first.items.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
        mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":["\\Seen","\\Flagged"],"remove":[]})).await?;
        mock.control(
            json!({"command":"expunge","account":"alpha@example.test","mailbox":"INBOX","uid":2}),
        )
        .await?;
        sync(&h, &mut mock, &account, false).await?;
        let changed = list(&h, &account).await?;
        assert_eq!(changed.items.len(), 2);
        let inbox = changed
            .items
            .iter()
            .find(|m| m.collections[0].name.as_deref() == Some("INBOX"))
            .ok_or("inbox absent")?;
        let data = h
            .mail()
            .get_message(v2::MessageRequest {
                account_id: account.clone(),
                message_id: inbox.id.clone(),
            })
            .await?
            .into_inner();
        let state: serde_json::Value = serde_json::from_str(&data.provider_json)?;
        let flags = state["flags"].as_array().ok_or("flags missing")?;
        assert!(flags.contains(&json!("\\Seen")) && flags.contains(&json!("\\Flagged")));
        let old_id = inbox.id.clone();
        let folder_id = inbox.collections[0].id.clone();
        mock.control(json!({"command":"uidvalidity","account":"alpha@example.test","mailbox":"INBOX","value":9100})).await?;
        sync(&h, &mut mock, &account, false).await?;
        let after = list(&h, &account).await?;
        assert_eq!(after.items.len(), 2);
        let inbox = after
            .items
            .iter()
            .find(|m| m.collections[0].name.as_deref() == Some("INBOX"))
            .unwrap();
        assert_ne!(inbox.id, old_id);
        assert_eq!(inbox.collections[0].id, folder_id);
        for folder in ["INBOX", "Archive"] {
            mock.control(json!({"command":"expunge","account":"alpha@example.test","mailbox":folder,"uid":1})).await?;
        }
        let fetches = mock.control(json!({"command":"snapshot"})).await?["requests"]
            ["imap UID FETCH"]
            .clone();
        sync(&h, &mut mock, &account, false).await?;
        let empty = list(&h, &account).await?;
        assert!(empty.items.is_empty());
        sync(&h, &mut mock, &account, false).await?;
        assert_eq!(
            list(&h, &account).await?.coverage.as_ref().unwrap().cursor,
            empty.coverage.as_ref().unwrap().cursor
        );
        assert_eq!(
            mock.control(json!({"command":"snapshot"})).await?["requests"]["imap UID FETCH"],
            fetches,
            "empty inventories must never issue FETCH"
        );
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn failed_imap_reads_preserve_previous_projection_and_credentials_through_restart(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = h
        .accounts()
        .connect_imap(request(&mock, false, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .ok_or("account missing")?
        .id;
    sync(&h, &mut mock, &account, true).await?;
    let original = list(&h, &account).await?;
    let ids = original
        .items
        .iter()
        .map(|m| m.id.clone())
        .collect::<Vec<_>>();
    mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":["\\Flagged"],"remove":[]})).await?;
    for (name, verb, phase, action) in [
        ("listing", "LIST", "before", "reject"),
        ("inventory", "UID SEARCH", "after", "disconnect"),
        ("literal", "UID FETCH", "after", "truncate"),
    ] {
        mock.control(json!({"command":"inject","name":name,"protocol":"imap","verb":verb,"phase":phase,"action":action})).await?;
        let failed = run(&h, &account, true).await?;
        assert_eq!(failed.state, "failed");
        assert!(matches!(
            failed.error_code.as_deref(),
            Some("unavailable" | "invalid_provider_response")
        ));
        assert_eq!(
            mock.control(json!({"command":"wait_fault","name":name}))
                .await?["entered"],
            true
        );
        let stale = list(&h, &account).await?;
        assert_eq!(stale.coverage, original.coverage);
        assert_eq!(
            stale.items.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
            ids
        );
        h.shutdown().await?;
        h.restart().await?;
        assert_eq!(list(&h, &account).await?.coverage, original.coverage);
        assert_eq!(
            h.accounts()
                .check_account(v2::AccountRequest {
                    account_id: account.clone()
                })
                .await?
                .into_inner()
                .state,
            "connected"
        );
    }
    sync(&h, &mut mock, &account, false).await?;
    assert_ne!(list(&h, &account).await?.coverage, original.coverage);
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn encoded_quoted_folders_and_identical_uids_stay_account_scoped() -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let alpha = h
        .accounts()
        .connect_imap(request(&mock, true, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .ok_or("alpha missing")?
        .id;
    let beta = h
        .accounts()
        .connect_imap(request(&mock, false, "beta@example.test").await?)
        .await?
        .into_inner()
        .account
        .ok_or("beta missing")?
        .id;
    let folder = "旅行 & \"quotes\" \\ archive";
    mock.control(
        json!({"command":"folder_create","account":"alpha@example.test","mailbox":folder}),
    )
    .await?;
    mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"INBOX","uid":1,"destination":folder})).await?;
    sync(&h, &mut mock, &alpha, true).await?;
    sync(&h, &mut mock, &beta, true).await?;
    let a = list(&h, &alpha).await?;
    let b = list(&h, &beta).await?;
    assert_eq!(a.items.len(), 3);
    assert_eq!(b.items.len(), 2);
    let copy = a
        .items
        .iter()
        .find(|m| m.collections[0].name.as_deref() == Some(folder))
        .ok_or("encoded folder missing")?;
    assert_eq!(
        h.mail()
            .get_message(v2::MessageRequest {
                account_id: beta.clone(),
                message_id: copy.id.clone()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::NotFound
    );
    let folder_id = copy.collections[0].id.clone();
    let message_id = copy.id.clone();
    mock.control(json!({"command":"folder_rename","account":"alpha@example.test","mailbox":folder,"destination":"Renamed"})).await?;
    sync(&h, &mut mock, &alpha, false).await?;
    let after = list(&h, &alpha).await?;
    assert_eq!(after.items.len(), 3);
    let renamed = after
        .items
        .iter()
        .find(|m| m.collections[0].name.as_deref() == Some("Renamed"))
        .unwrap();
    assert_ne!(renamed.id, message_id);
    assert_ne!(renamed.collections[0].id, folder_id);
    assert_eq!(list(&h, &beta).await?.items, b.items);
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
