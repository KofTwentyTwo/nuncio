use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;
pub(super) async fn list(h: &E2eHarness, account: &str) -> Result<Value, TestError> {
    let result = h
        .cli(&["--json", "mail", "list", "--account", account])
        .await?;
    assert_eq!(result.status, 0);
    Ok(result.json()?["result"].clone())
}
pub(super) fn arm(h: &E2eHarness, name: &str) -> Result<(), TestError> {
    let directory = h.artifacts.join("barriers");
    for extension in ["entered", "release"] {
        match std::fs::remove_file(directory.join(format!("{name}.{extension}"))) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    std::fs::write(directory.join(format!("{name}.arm")), [])?;
    Ok(())
}
pub(super) async fn wait(h: &E2eHarness, name: &str) -> Result<(), TestError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !h
        .artifacts
        .join("barriers")
        .join(format!("{name}.entered"))
        .exists()
    {
        if tokio::time::Instant::now() > deadline {
            return Err("IMAP worker never reached crash boundary".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Ok(())
}
pub async fn reads(
    h: &mut E2eHarness,
    mock: &mut MockMailPlus,
    account: &str,
) -> Result<(), TestError> {
    let output = h
        .cli(&["--json", "sync", "--account", account, "--wait"])
        .await?;
    assert_eq!(
        output.status,
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first = list(h, account).await?;
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let item = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == "Independent MailPlus fixture 0")
        .unwrap();
    let id = item["id"].as_str().unwrap();
    let remote = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}))
        .await?;
    let expected = STANDARD.decode(remote["raw_base64"].as_str().ok_or("remote MIME missing")?)?;
    let raw = h.artifacts.join("imap-download.eml");
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "raw",
            "--account",
            account,
            "--message",
            id,
            "--output",
            raw.to_str().unwrap()
        ])
        .await?
        .status,
        0
    );
    assert_eq!(std::fs::read(raw)?, expected);
    let read = h
        .cli(&[
            "--json",
            "mail",
            "read",
            "--account",
            account,
            "--message",
            id,
        ])
        .await?;
    assert_eq!(read.status, 0);
    let detail = read.json()?;
    let attachment = detail["result"]["attachments"][0]["id"]
        .as_str()
        .ok_or("attachment absent")?;
    let file = h.artifacts.join("imap-download.pdf");
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "attachment",
            "--account",
            account,
            "--message",
            id,
            "--attachment",
            attachment,
            "--output",
            file.to_str().unwrap()
        ])
        .await?
        .status,
        0
    );
    assert_eq!(
        std::fs::read(file)?,
        b"%PDF-1.4\nSynthetic independent fixture\n%%EOF\n"
    );
    mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":["\\Flagged"],"remove":[]})).await?;
    let fetched = h
        .cli(&[
            "--json",
            "mail",
            "fetch",
            "--account",
            account,
            "--message",
            id,
            "--wait",
        ])
        .await?;
    assert_eq!(fetched.status, 0);
    assert_eq!(fetched.json()?["result"]["mode"], "fetch");
    let after_fetch = list(h, account).await?;
    assert_eq!(after_fetch["items"], first["items"]);
    assert_eq!(after_fetch["coverage"], first["coverage"]);
    let read = h
        .cli(&[
            "--json",
            "mail",
            "read",
            "--account",
            account,
            "--message",
            id,
        ])
        .await?;
    assert_eq!(read.status, 0);
    let detail = read.json()?;
    let provider: Value = serde_json::from_str(
        detail["result"]["provider_json"]
            .as_str()
            .ok_or("provider state missing")?,
    )?;
    assert!(provider["flags"]
        .as_array()
        .unwrap()
        .contains(&json!("\\Flagged")));
    let source = h.artifacts.join("imap-durable-draft.json");
    std::fs::write(&source,br#"{"to":[{"address":"recipient@example.test"}],"subject":"Retain through reset","text":"Durable offline work"}"#)?;
    let draft = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            account,
            "--file",
            source.to_str().unwrap(),
        ])
        .await?;
    assert_eq!(draft.status, 0);
    let saved = draft.json()?["result"].clone();
    let draft_id = saved["id"].as_str().unwrap();
    for (ordinal, name, published) in [
        (0, "imap-before-message-stage", false),
        (1, "imap-before-promotion", false),
        (2, "imap-after-promotion", true),
    ] {
        let before = list(h, account).await?;
        mock.control(json!({"command":"uidvalidity","account":"alpha@example.test","mailbox":"INBOX","value":9200+ordinal})).await?;
        std::fs::write(h.artifacts.join("barriers").join(format!("{name}.arm")), [])?;
        let started = h
            .cli(&["--json", "sync", "--account", account, "--full"])
            .await?;
        assert_eq!(started.status, 0);
        wait(h, name).await?;
        let interrupted = list(h, account).await?;
        if published {
            assert_ne!(
                interrupted["coverage"]["cursor"],
                before["coverage"]["cursor"]
            );
        } else {
            assert_eq!(interrupted["items"], before["items"]);
            assert_eq!(interrupted["coverage"], before["coverage"]);
        }
        h.force_kill().await?;
        h.restart().await?;
        let restored = list(h, account).await?;
        assert_eq!(restored["items"], interrupted["items"]);
        assert_eq!(restored["coverage"], interrupted["coverage"]);
        let result = h
            .cli(&["--json", "sync", "--account", account, "--wait"])
            .await?;
        assert_eq!(result.status, 0);
        let after = list(h, account).await?;
        assert_eq!(after["items"].as_array().unwrap().len(), 2);
        assert_ne!(after["items"][0]["id"], before["items"][0]["id"]);
        let draft = h
            .cli(&[
                "--json",
                "mail",
                "draft",
                "show",
                "--account",
                account,
                "--draft",
                draft_id,
            ])
            .await?;
        assert_eq!(draft.status, 0);
        assert_eq!(draft.json()?["result"], saved);
        let remote = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        assert_eq!(remote["messages"].as_array().unwrap().len(), 2);
        assert_eq!(remote["uidvalidity"], 9200 + ordinal);
    }
    Ok(())
}
