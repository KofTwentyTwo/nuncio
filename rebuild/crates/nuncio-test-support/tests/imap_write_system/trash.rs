use super::*;

#[tokio::test]
async fn restore_requires_known_origin_and_retained_folder_identity_and_epoch(
) -> Result<(), TestError> {
    for failure in ["unknown", "retired", "epoch"] {
        let mut mock =
            MockMailPlus::start(if failure == "epoch" { &["MOVE"] } else { &[] }).await?;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
        let account = h
            .accounts()
            .connect_imap(request(&mock, true, "alpha@example.test").await?)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id;
        let source_folder = if failure == "unknown" {
            "Trash"
        } else {
            "Archive"
        };
        mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"INBOX","uid":1,"destination":source_folder})).await?;
        sync(&h, &mut mock, &account, true).await?;
        let messages = list(&h, &account).await?;
        let source = messages
            .items
            .iter()
            .find(|m| m.collections[0].name.as_deref() == Some(source_folder))
            .unwrap()
            .id
            .clone();
        if failure != "unknown" {
            let queued = h
                .mail()
                .change_message(v2::ChangeMessageRequest {
                    account_id: account.clone(),
                    message_id: source,
                    request_id: "ccf99380-f4f9-4a22-a180-2b33ba2ab754".into(),
                    action: Some(v2::change_message_request::Action::Trash(
                        v2::MailTrashChange { trashed: true },
                    )),
                })
                .await?
                .into_inner();
            applied(&h, &account, &queued.id).await?;
            if failure == "retired" {
                mock.control(json!({"command":"folder_delete","account":"alpha@example.test","mailbox":"Archive"})).await?;
            } else {
                mock.control(json!({"command":"uidvalidity","account":"alpha@example.test","mailbox":"Archive","value":9047})).await?;
            }
        }
        h.shutdown().await?;
        h.restart().await?;
        sync(&h, &mut mock, &account, true).await?;
        let before = list(&h, &account).await?;
        let source = before
            .items
            .iter()
            .find(|m| m.collections[0].name.as_deref() == Some("Trash"))
            .unwrap()
            .id
            .clone();
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        let result = h
            .mail()
            .change_message(v2::ChangeMessageRequest {
                account_id: account.clone(),
                message_id: source,
                request_id: "015a3d4e-630c-4bb8-a5ba-0c9f5e0cdd5e".into(),
                action: Some(v2::change_message_request::Action::Trash(
                    v2::MailTrashChange { trashed: false },
                )),
            })
            .await;
        assert_eq!(
            result.unwrap_err().code(),
            tonic::Code::FailedPrecondition,
            "{failure}"
        );
        assert_eq!(list(&h, &account).await?.items, before.items);
        let after = mock.control(json!({"command":"snapshot"})).await?;
        for verb in [
            "imap UID MOVE",
            "imap UID COPY",
            "imap UID STORE",
            "imap UID EXPUNGE",
            "imap EXPUNGE",
        ] {
            assert_eq!(
                after["requests"][verb], snapshot["requests"][verb],
                "{failure}:{verb}"
            );
        }
        let trash = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Trash"}))
            .await?;
        assert_eq!(trash["messages"].as_array().unwrap().len(), 1);
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn copies_into_and_within_trash_retain_origin_without_merging_identical_messages(
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
    sync(&h, &mut mock, &account, true).await?;
    let collections = h
        .mail()
        .list_collections(v2::ListCollectionsRequest {
            account_id: account.clone(),
        })
        .await?
        .into_inner();
    let trash = collections
        .items
        .into_iter()
        .find(|c| c.name.as_deref() == Some("Trash"))
        .unwrap()
        .id;
    let original = list(&h, &account)
        .await?
        .items
        .into_iter()
        .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
        .unwrap()
        .id;
    let mut source = original.clone();
    let mut copies = Vec::new();
    for index in 0..2 {
        let queued = h
            .mail()
            .change_message(v2::ChangeMessageRequest {
                account_id: account.clone(),
                message_id: source,
                request_id: format!("d2ff25ba-473e-40a2-a7ca-{index:012}"),
                action: Some(v2::change_message_request::Action::Copy(
                    v2::MailFolderChange {
                        destination_collection_id: trash.clone(),
                    },
                )),
            })
            .await?
            .into_inner();
        applied(&h, &account, &queued.id).await?;
        source = list(&h, &account)
            .await?
            .items
            .into_iter()
            .find(|m| m.collections[0].name.as_deref() == Some("Trash") && !copies.contains(&m.id))
            .unwrap()
            .id;
        copies.push(source.clone());
    }
    h.shutdown().await?;
    h.restart().await?;
    sync(&h, &mut mock, &account, true).await?;
    let queued = h
        .mail()
        .change_message(v2::ChangeMessageRequest {
            account_id: account.clone(),
            message_id: source,
            request_id: "8ea64836-024b-4cb0-9b4e-feecf1a880cd".into(),
            action: Some(v2::change_message_request::Action::Trash(
                v2::MailTrashChange { trashed: false },
            )),
        })
        .await?
        .into_inner();
    applied(&h, &account, &queued.id).await?;
    let current = list(&h, &account).await?;
    assert_eq!(current.items.len(), 4);
    assert!(current.items.iter().any(|m| m.id == original));
    assert!(current.items.iter().any(|m| m.id == copies[0]));
    assert!(!current.items.iter().any(|m| m.id == copies[1]));
    let restored = current
        .items
        .iter()
        .find(|m| {
            m.collections[0].name.as_deref() == Some("INBOX")
                && m.id != original
                && m.subject.as_deref() == Some("Independent MailPlus fixture 0")
        })
        .unwrap();
    assert!(!copies.contains(&restored.id));
    let raw = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}))
        .await?;
    let restored_raw = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":3}))
        .await?;
    assert_eq!(restored_raw["raw_base64"], raw["raw_base64"]);
    let remote_trash = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Trash"}))
        .await?;
    assert_eq!(remote_trash["messages"].as_array().unwrap().len(), 1);
    assert_eq!(remote_trash["messages"][0]["uid"], 1);
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["requests"]["imap UID COPY"], 2);
    assert_eq!(snapshot["requests"]["imap UID MOVE"], 1);
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
