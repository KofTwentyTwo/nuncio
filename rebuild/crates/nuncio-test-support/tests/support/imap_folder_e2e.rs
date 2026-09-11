use super::imap_read_e2e::list;
use super::imap_transfer_e2e::connect;
use super::*;

#[tokio::test]
async fn explicit_copy_and_move_keep_distinct_placements_and_encode_destination_names(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let account = connect(&h, &mock).await?;
    let folder = "旅行 & \"quotes\" \\ target";
    mock.control(
        json!({"command":"folder_create","account":"alpha@example.test","mailbox":folder}),
    )
    .await?;
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?
            .status,
        0
    );
    let folders = h
        .cli(&["--json", "mail", "collections", "--account", &account])
        .await?;
    assert_eq!(folders.status, 0);
    let folders = folders.json()?;
    let collection = |name: &str| {
        folders["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
    };
    let inbox_id = collection("INBOX");
    let target_id = collection(folder);
    let original = list(&h, &account).await?;
    let source_id = original["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == "Independent MailPlus fixture 0")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let raw = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}))
        .await?;
    let mut message = source_id.to_owned();
    let mut first_copy = String::new();
    for (index, action, destination) in [
        (0, "copy", target_id),
        (1, "copy", target_id),
        (2, "move", inbox_id),
        (3, "move", inbox_id),
    ] {
        let input = h.artifacts.join(format!("folder-{index}.json"));
        std::fs::write(
            &input,
            json!({"schema_version":1,"action":action,"destination_collection_id":destination})
                .to_string(),
        )?;
        let request = format!("aa57cbe3-7c6c-4718-a064-{index:012}");
        let args = [
            "--json",
            "mail",
            "change",
            "--account",
            &account,
            "--message",
            &message,
            "--request-id",
            &request,
            "--file",
            input.to_str().unwrap(),
            "--wait",
        ];
        let result = h.cli(&args).await?;
        assert_eq!(result.status, 0, "step{index}: {:?}", result.json());
        let result = result.json()?;
        assert_eq!(result["result"]["state"], "applied");
        assert_eq!(
            h.cli(&args).await?.json()?["result"]["id"],
            result["result"]["id"]
        );
        let current = list(&h, &account).await?;
        assert_eq!(
            current["items"].as_array().unwrap().len(),
            if index == 0 { 3 } else { 4 }
        );
        assert!(current["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == source_id));
        if index == 0 {
            first_copy = current["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["collections"][0]["name"] == folder)
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_ne!(first_copy, source_id);
            message = first_copy.clone();
        } else if index == 2 {
            assert!(!current["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == first_copy));
            message = current["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| {
                    m["collections"][0]["name"] == "INBOX"
                        && m["id"] != source_id
                        && m["subject"] == "Independent MailPlus fixture 0"
                })
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
        } else if index == 3 {
            assert!(current["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == message));
        }
    }
    let remote = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":folder}))
        .await?;
    assert_eq!(remote["messages"].as_array().unwrap().len(), 1);
    assert_eq!(remote["messages"][0]["uid"], 2);
    let copy = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":folder,"uid":2}))
        .await?;
    assert_eq!(copy["raw_base64"], raw["raw_base64"]);
    let moved = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":3}))
        .await?;
    assert_eq!(moved["raw_base64"], raw["raw_base64"]);
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["requests"]["imap UID COPY"], 2);
    assert_eq!(snapshot["accepted"]["imap UID COPY"], 2);
    assert_eq!(snapshot["requests"]["imap UID MOVE"], 1);
    assert_eq!(snapshot["accepted"]["imap UID MOVE"], 1);
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
