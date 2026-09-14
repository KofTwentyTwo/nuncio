use super::*;

#[tokio::test]
async fn daemon_trace_keeps_imap_smtp_credentials_and_mime_out_of_raw_logs() -> Result<(), TestError>
{
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    h.shutdown().await?;
    h.restart_with_log_level(Some("trace")).await?;
    let account = imap_transfer_e2e::connect(&h, &mock).await?;
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?
            .status,
        0
    );
    let file = h.artifacts.join("private-log-draft.json");
    std::fs::write(&file, json!({"to":[{"address":"beta@example.test"}],"subject":"imap-subject-canary","text":"smtp-body-canary"}).to_string())?;
    let draft = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            file.to_str().unwrap(),
        ])
        .await?;
    assert_eq!(draft.status, 0);
    let draft = draft.json()?["result"]["id"].as_str().unwrap().to_owned();
    let sent = h
        .cli(&[
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "62c04dda-3dab-4514-aad5-d95b3b0de052",
            "--wait",
        ])
        .await?;
    assert_eq!(sent.status, 0);
    assert_eq!(sent.json()?["result"]["state"], "applied");
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 1);
    let folder = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
        .await?;
    assert_eq!(folder["messages"].as_array().unwrap().len(), 1);
    h.shutdown().await?;
    // Inspect before E2eHarness::drop redacts its retained artifacts.
    let logs = std::fs::read_to_string(h.artifacts.join("daemon-2.stderr.log"))?;
    for expected in [
        "DEBUG",
        "Account connected",
        "scope=imap",
        "Sync completed",
        "state=applied",
    ] {
        assert!(logs.contains(expected), "missing {expected}");
    }
    let password = mock.credential("alpha@example.test")?;
    for forbidden in [
        &password,
        "alpha@example.test",
        "beta@example.test",
        "imap-subject-canary",
        "smtp-body-canary",
        "AUTH PLAIN",
        "Content-Type:",
        "Message-ID:",
    ] {
        assert!(
            !logs.contains(forbidden),
            "private data appeared in raw daemon logs"
        );
    }
    mock.shutdown().await?;
    Ok(())
}
