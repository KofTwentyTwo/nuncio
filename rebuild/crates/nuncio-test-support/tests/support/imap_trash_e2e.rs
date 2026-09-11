use super::imap_read_e2e::{arm, list, wait};
use super::imap_transfer_e2e::connect;
use super::*;

#[tokio::test]
async fn trash_and_restore_retain_original_folder_across_crash_and_full_resync(
) -> Result<(), TestError> {
    for hidden in [vec![], vec!["MOVE"]] {
        scenario(&hidden).await?;
    }
    Ok(())
}
async fn scenario(hidden: &[&str]) -> Result<(), TestError> {
    let native = hidden.is_empty();
    let mut mock = MockMailPlus::start(hidden).await?;
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let account = connect(&h, &mock).await?;
    mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"INBOX","uid":1,"destination":"Archive"})).await?;
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?
            .status,
        0
    );
    let before = list(&h, &account).await?;
    let original = before["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["collections"][0]["name"] == "Archive")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let raw = mock
        .control(
            json!({"command":"raw","account":"alpha@example.test","mailbox":"Archive","uid":1}),
        )
        .await?;
    let mut source = original.to_owned();
    for (index, trashed) in [(0, true), (1, true), (2, false)] {
        let input = h.artifacts.join(format!("trash-{index}.json"));
        std::fs::write(
            &input,
            json!({"schema_version":1,"action":"trash","trashed":trashed}).to_string(),
        )?;
        let request = format!("cc276dac-6df2-4bd1-ad99-{index:012}");
        let args = [
            "--json",
            "mail",
            "change",
            "--account",
            &account,
            "--message",
            &source,
            "--request-id",
            &request,
            "--file",
            input.to_str().unwrap(),
        ];
        if index != 1 {
            arm(&h, "operation_before_receipt")?;
        }
        let queued = h.cli(&args).await?;
        assert_eq!(queued.status, 0, "step{index}: {:?}", queued.json());
        let queued = queued.json()?;
        if index != 1 {
            wait(&h, "operation_before_receipt").await?;
            let target = if trashed { "Trash" } else { "Archive" };
            let destination = mock
                .control(
                    json!({"command":"mailbox","account":"alpha@example.test","mailbox":target}),
                )
                .await?;
            assert_eq!(destination["messages"].as_array().unwrap().len(), 1);
            assert!(list(&h, &account).await?["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == source));
            h.force_kill().await?;
            h.restart().await?;
        }
        let mut waited = args.to_vec();
        waited.push("--wait");
        let applied = h.cli(&waited).await?;
        assert_eq!(applied.status, 0, "step{index}: {:?}", applied.json());
        assert_eq!(applied.json()?["result"]["id"], queued["result"]["id"]);
        assert_eq!(applied.json()?["result"]["state"], "applied");
        let count = if index == 2 { 2 } else { 1 };
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        let verb = if native {
            "imap UID MOVE"
        } else {
            "imap UID COPY"
        };
        assert_eq!(snapshot["requests"][verb], count);
        assert_eq!(snapshot["accepted"][verb], count);
        assert_eq!(
            snapshot["requests"]["imap UID EXPUNGE"]
                .as_u64()
                .unwrap_or(0),
            if native { 0 } else { count }
        );
        assert_eq!(
            snapshot["requests"]["imap EXPUNGE"].as_u64().unwrap_or(0),
            0
        );
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--full", "--wait"])
                .await?
                .status,
            0
        );
        let current = list(&h, &account).await?;
        assert_eq!(current["items"].as_array().unwrap().len(), 3);
        let destination = if trashed { "Trash" } else { "Archive" };
        let next = current["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["collections"][0]["name"] == destination)
            .unwrap()["id"]
            .as_str()
            .unwrap();
        if index == 1 {
            assert_eq!(next, source);
        } else {
            assert_ne!(next, source);
        }
        source = next.to_owned();
    }
    assert_ne!(source, original);
    let restored = mock
        .control(
            json!({"command":"raw","account":"alpha@example.test","mailbox":"Archive","uid":2}),
        )
        .await?;
    assert_eq!(restored["raw_base64"], raw["raw_base64"]);
    let trash = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Trash"}))
        .await?;
    assert!(trash["messages"].as_array().unwrap().is_empty());
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
