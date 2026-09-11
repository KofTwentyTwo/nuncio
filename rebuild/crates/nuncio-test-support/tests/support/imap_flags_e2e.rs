use super::imap_read_e2e::{list, wait};
use super::*;

pub async fn flags(
    h: &mut E2eHarness,
    mock: &mut MockMailPlus,
    account: &str,
) -> Result<(), TestError> {
    let messages = list(h, account).await?;
    let message = messages["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == "Independent MailPlus fixture 0")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    for (ordinal, boundary) in [
        "operation_after_attempt",
        "operation_before_receipt",
        "remote_ack",
    ]
    .into_iter()
    .enumerate()
    {
        mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":[],"remove":["\\Seen"]})).await?;
        assert_eq!(
            h.cli(&[
                "--json",
                "mail",
                "fetch",
                "--account",
                account,
                "--message",
                message,
                "--wait"
            ])
            .await?
            .status,
            0
        );
        if boundary == "remote_ack" {
            mock.control(json!({"command":"inject","name":"flags-accepted","protocol":"imap","verb":"UID STORE","phase":"after","action":"withhold"})).await?;
        } else {
            std::fs::write(
                h.artifacts.join("barriers").join(format!("{boundary}.arm")),
                [],
            )?;
        }
        let file = h.artifacts.join("imap-flag-intent.json");
        std::fs::write(
            &file,
            br#"{"schema_version":1,"action":"read","read":true}"#,
        )?;
        let request = format!("9cebbef2-761c-426a-a59a-{ordinal:012}");
        let args = [
            "--json",
            "mail",
            "change",
            "--account",
            account,
            "--message",
            message,
            "--request-id",
            &request,
            "--file",
            file.to_str().unwrap(),
        ];
        let queued = h.cli(&args).await?;
        assert_eq!(queued.status, 0);
        let queued = queued.json()?;
        if boundary == "remote_ack" {
            assert_eq!(
                mock.control(json!({"command":"wait_fault","name":"flags-accepted"}))
                    .await?["entered"],
                true
            );
        } else {
            wait(h, boundary).await?;
        }
        let remote = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        assert_eq!(
            remote["messages"][0]["flags"]
                .as_array()
                .unwrap()
                .contains(&json!("\\Seen")),
            boundary != "operation_after_attempt"
        );
        let local = h
            .cli(&[
                "--json",
                "mail",
                "read",
                "--account",
                account,
                "--message",
                message,
            ])
            .await?;
        assert_eq!(local.status, 0);
        let state: serde_json::Value =
            serde_json::from_str(local.json()?["result"]["provider_json"].as_str().unwrap())?;
        assert!(!state["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("\\Seen")));
        h.force_kill().await?;
        if boundary == "remote_ack" {
            mock.control(json!({"command":"release","name":"flags-accepted"}))
                .await?;
        }
        mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":["\\Answered"],"remove":[]})).await?;
        h.restart().await?;
        let mut waited = args.to_vec();
        waited.push("--wait");
        let done = h.cli(&waited).await?;
        assert_eq!(done.status, 0, "{boundary}: {:?}", done.json());
        assert_eq!(done.json()?["result"]["id"], queued["result"]["id"]);
        assert_eq!(done.json()?["result"]["state"], "applied");
        let observation = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(observation["requests"]["imap UID STORE"], ordinal + 1);
        assert_eq!(observation["accepted"]["imap UID STORE"], ordinal + 1);
        let remote = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        let remote_flags = remote["messages"][0]["flags"].as_array().unwrap();
        assert!(remote_flags.contains(&json!("\\Seen")));
        assert!(remote_flags.contains(&json!("\\Answered")));
        let local = h
            .cli(&[
                "--json",
                "mail",
                "read",
                "--account",
                account,
                "--message",
                message,
            ])
            .await?;
        assert_eq!(local.status, 0);
        let state: serde_json::Value =
            serde_json::from_str(local.json()?["result"]["provider_json"].as_str().unwrap())?;
        assert_eq!(state["flags"], remote["messages"][0]["flags"]);
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    }
    Ok(())
}
