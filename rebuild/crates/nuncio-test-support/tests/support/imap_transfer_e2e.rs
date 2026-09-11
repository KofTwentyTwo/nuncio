use super::imap_read_e2e::{arm, list, wait};
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};

pub async fn archive(
    h: &mut E2eHarness,
    mock: &mut MockMailPlus,
    account: &str,
) -> Result<(), TestError> {
    let before = list(h, account).await?;
    let source = before["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == "Independent MailPlus fixture 0")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let remote = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}))
        .await?;
    let raw = STANDARD.decode(remote["raw_base64"].as_str().unwrap())?;
    arm(h, "operation_before_receipt")?;
    let input = h.artifacts.join("imap-archive.json");
    std::fs::write(&input, br#"{"schema_version":1,"action":"archive"}"#)?;
    let args = [
        "--json",
        "mail",
        "change",
        "--account",
        account,
        "--message",
        source,
        "--request-id",
        "78241409-6a4f-4e54-9c84-a4c2f7b4b05f",
        "--file",
        input.to_str().unwrap(),
    ];
    let queued = h.cli(&args).await?;
    assert_eq!(queued.status, 0);
    let queued = queued.json()?;
    wait(h, "operation_before_receipt").await?;
    let inbox = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
        .await?;
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["messages"][0]["uid"], 2);
    assert_eq!(list(h, account).await?["items"], before["items"]);
    h.force_kill().await?;
    h.restart().await?;
    let mut waited = args.to_vec();
    waited.push("--wait");
    let done = h.cli(&waited).await?;
    assert_eq!(done.status, 0, "{:?}", done.json());
    assert_eq!(done.json()?["result"]["id"], queued["result"]["id"]);
    assert_eq!(done.json()?["result"]["state"], "applied");
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["requests"]["imap UID MOVE"], 1);
    assert_eq!(snapshot["accepted"]["imap UID MOVE"], 1);
    let destination = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Archive"}))
        .await?;
    assert_eq!(destination["messages"].as_array().unwrap().len(), 1);
    let after = list(h, account).await?;
    assert_eq!(after["items"].as_array().unwrap().len(), 2);
    let copied = after["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == "Independent MailPlus fixture 0")
        .unwrap();
    assert_ne!(copied["id"], source);
    assert_eq!(copied["collections"][0]["name"], "Archive");
    let output = h.artifacts.join("imap-archived.eml");
    let downloaded = h
        .cli(&[
            "--json",
            "mail",
            "raw",
            "--account",
            account,
            "--message",
            copied["id"].as_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .await?;
    assert_eq!(downloaded.status, 0);
    assert_eq!(std::fs::read(output)?, raw);
    // A second, externally created placement has identical MIME but its own UID.
    mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"Archive","uid":1,"destination":"INBOX"})).await?;
    assert_eq!(
        h.cli(&["--json", "sync", "--account", account, "--wait"])
            .await?
            .status,
        0
    );
    let seeded = list(h, account).await?;
    let next_source = seeded["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| {
            m["subject"] == "Independent MailPlus fixture 0"
                && m["collections"][0]["name"] == "INBOX"
        })
        .unwrap()["id"]
        .as_str()
        .unwrap();
    arm(h, "operation_after_attempt")?;
    let args = [
        "--json",
        "mail",
        "change",
        "--account",
        account,
        "--message",
        next_source,
        "--request-id",
        "a70c4f48-39e4-4381-a7a0-cde33538bceb",
        "--file",
        input.to_str().unwrap(),
    ];
    let queued = h.cli(&args).await?;
    assert_eq!(queued.status, 0);
    let queued = queued.json()?;
    wait(h, "operation_after_attempt").await?;
    assert_eq!(
        mock.control(json!({"command":"snapshot"})).await?["requests"]["imap UID MOVE"],
        1
    );
    h.force_kill().await?;
    h.restart().await?;
    let mut args = args.to_vec();
    args.push("--wait");
    let done = h.cli(&args).await?;
    assert_eq!(done.status, 0, "{:?}", done.json());
    assert_eq!(done.json()?["result"]["id"], queued["result"]["id"]);
    assert_eq!(
        mock.control(json!({"command":"snapshot"})).await?["requests"]["imap UID MOVE"],
        2
    );
    let remote = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Archive"}))
        .await?;
    assert_eq!(remote["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        remote["messages"][0]["sha256"],
        remote["messages"][1]["sha256"]
    );
    let current = list(h, account).await?;
    assert_eq!(
        current["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["subject"] == "Independent MailPlus fixture 0"
                && m["collections"][0]["name"] == "Archive")
            .count(),
        2
    );
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    Ok(())
}

pub(super) async fn connect(h: &E2eHarness, mock: &MockMailPlus) -> Result<String, TestError> {
    connect_with_policy(h, mock, "client_append").await
}
pub(super) async fn connect_with_policy(
    h: &E2eHarness,
    mock: &MockMailPlus,
    policy: &str,
) -> Result<String, TestError> {
    let config = json!({"schema_version":1,"address":"alpha@example.test","imap":{"host":"127.0.0.1","port":mock.ready.ports.imaps,"tls":"implicit","username":"alpha@example.test"},"smtp":{"host":"127.0.0.1","port":mock.ready.ports.smtp,"tls":"start_tls","username":"alpha@example.test"},"sent_policy":policy,"sent_folder":"Sent","archive_folder":"Archive","trash_folder":"Trash","trusted_ca_pem":tokio::fs::read_to_string(&mock.ready.ca_file).await?});
    let path = h.artifacts.join("imap-config.json");
    std::fs::write(&path, config.to_string())?;
    let password = mock.credential("alpha@example.test")?;
    let credentials = zeroize::Zeroizing::new(
        json!({"imap_password":password,"smtp_password":password}).to_string(),
    );
    let response = h
        .cli_with_stdin(
            &[
                "--json",
                "account",
                "connect-imap",
                "--config",
                path.to_str().unwrap(),
                "--credentials-stdin",
            ],
            credentials.as_bytes(),
        )
        .await?;
    assert_eq!(response.status, 0);
    Ok(response.json()?["result"]["account"]["id"]
        .as_str()
        .unwrap()
        .to_owned())
}

#[tokio::test]
async fn archive_substeps_survive_sigkill_without_recopying_or_expunge_of_unrelated_messages(
) -> Result<(), TestError> {
    for boundary in [
        "operation_after_imap_copy",
        "UID STORE",
        "UID EXPUNGE",
        "UID COPY",
        "UID MOVE",
    ] {
        let native = boundary == "UID MOVE";
        let known_copy = !matches!(boundary, "UID COPY" | "UID MOVE");
        let mut mock = MockMailPlus::start(if native { &[] } else { &["MOVE"] }).await?;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let account = connect(&h, &mock).await?;
        mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":2,"add":["\\Deleted"],"remove":[]})).await?;
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await?
                .status,
            0
        );
        let before = list(&h, &account).await?;
        let source = before["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["subject"] == "Independent MailPlus fixture 0")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let source_raw = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}),
            )
            .await?;
        if boundary.starts_with("operation_") {
            arm(&h, boundary)?;
        } else {
            mock.control(json!({"command":"inject","name":"transfer-accepted","protocol":"imap","verb":boundary,"phase":"after","action":"withhold"})).await?;
        }
        let file = h.artifacts.join("archive.json");
        std::fs::write(&file, br#"{"schema_version":1,"action":"archive"}"#)?;
        let args = [
            "--json",
            "mail",
            "change",
            "--account",
            &account,
            "--message",
            source,
            "--request-id",
            "e00998d9-0833-453a-a272-0c3b1de57ca6",
            "--file",
            file.to_str().unwrap(),
        ];
        let queued = h.cli(&args).await?;
        assert_eq!(queued.status, 0);
        let queued = queued.json()?;
        if boundary.starts_with("operation_") {
            wait(&h, boundary).await?;
        } else {
            assert_eq!(
                mock.control(json!({"command":"wait_fault","name":"transfer-accepted"}))
                    .await?["entered"],
                true
            );
        }
        let inbox = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        let source_removed = matches!(boundary, "UID MOVE" | "UID EXPUNGE");
        assert_eq!(
            inbox["messages"].as_array().unwrap().len(),
            if source_removed { 1 } else { 2 },
            "{boundary}"
        );
        assert!(inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["uid"] == 2));
        let destination = mock
            .control(
                json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Archive"}),
            )
            .await?;
        assert_eq!(destination["messages"].as_array().unwrap().len(), 1);
        let copied = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"Archive","uid":1}),
            )
            .await?;
        assert_eq!(copied["raw_base64"], source_raw["raw_base64"]);
        assert_eq!(list(&h, &account).await?["items"], before["items"]);
        h.force_kill().await?;
        if !boundary.starts_with("operation_") {
            mock.control(json!({"command":"release","name":"transfer-accepted"}))
                .await?;
        }
        h.restart().await?;
        let mut waited = args.to_vec();
        waited.push("--wait");
        let done = h.cli(&waited).await?;
        assert_eq!(
            done.status,
            if known_copy { 0 } else { 5 },
            "{boundary}: {:?}",
            done.json()
        );
        let operation = h
            .cli(&[
                "--json",
                "operation",
                "show",
                "--account",
                &account,
                "--operation",
                queued["result"]["id"].as_str().unwrap(),
            ])
            .await?;
        assert_eq!(operation.status, 0);
        let op = operation.json()?;
        assert_eq!(
            op["result"]["state"],
            if known_copy { "applied" } else { "uncertain" }
        );
        assert_eq!(op["result"]["needs_reconciliation"], false);
        if !known_copy {
            assert_eq!(op["result"]["error_code"], "imap_copy_identity_unknown");
            // Reopening an unresolved operation must not silently give it another COPY.
            h.force_kill().await?;
            h.restart().await?;
            assert_eq!(h.cli(&waited).await?.status, 5);
            assert_eq!(list(&h, &account).await?["items"], before["items"]);
        }
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        let verb = if native {
            "imap UID MOVE"
        } else {
            "imap UID COPY"
        };
        assert_eq!(snapshot["requests"][verb], 1);
        assert_eq!(snapshot["accepted"][verb], 1);
        assert_eq!(
            snapshot["requests"]["imap EXPUNGE"].as_u64().unwrap_or(0),
            0
        );
        assert_eq!(
            snapshot["requests"]["imap UID EXPUNGE"]
                .as_u64()
                .unwrap_or(0),
            u64::from(known_copy)
        );
        let inbox = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        assert_eq!(
            inbox["messages"].as_array().unwrap().len(),
            if known_copy || native { 1 } else { 2 }
        );
        assert!(inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["uid"] == 2));
        let destination = mock
            .control(
                json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Archive"}),
            )
            .await?;
        assert_eq!(destination["messages"].as_array().unwrap().len(), 1);
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
