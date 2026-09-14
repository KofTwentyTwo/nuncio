use nuncio_test_support::{
    google::Seed,
    process::{CliOutput, E2eHarness},
    TestError,
};
use serde_json::json;

#[track_caller]
fn human(output: CliOutput) -> String {
    assert_eq!(
        output.status,
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.trim_start().starts_with(['{', '[']), "{text}");
    assert!(!text.contains(['\u{1b}', '\u{9b}', '\u{202e}']));
    text
}

#[tokio::test]
async fn readable_cli_runs_mail_calendar_drafts_and_send_with_independent_effects(
) -> Result<(), TestError> {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let account = h.connect_google("alpha@example.test").await?;
    assert!(human(h.cli(&["system", "status"]).await?).contains("API version:"));
    let accounts = human(h.cli(&["account", "list"]).await?);
    assert!(accounts.contains(&account));
    assert!(accounts.contains("alpha@example.test"));
    let unsynced = human(h.cli(&["mail", "list", "--account", &account]).await?);
    assert!(unsynced.contains("No completed local mail snapshot"));
    assert!(unsynced.contains("system status"));
    let machine = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await?
        .json()?;
    assert_eq!(machine["result"]["coverage"]["state"], "unavailable");
    assert_eq!(machine["result"]["items"], json!([]));
    assert!(
        human(h.cli(&["sync", "--account", &account, "--wait"]).await?)
            .contains("State: succeeded")
    );
    assert!(
        human(h.cli(&["mail", "list", "--account", &account]).await?)
            .contains("Subject: Multipart fixture")
    );
    assert!(human(
        h.cli(&[
            "mail",
            "search",
            "--account",
            &account,
            "--query",
            "searchable"
        ])
        .await?
    )
    .contains("Multipart fixture"));
    let empty_search = human(
        h.cli(&[
            "mail",
            "search",
            "--account",
            &account,
            "--query",
            "notpresentanywhere",
        ])
        .await?,
    );
    assert!(empty_search.contains("No items"));
    assert!(!empty_search.contains("No completed local mail snapshot"));
    let messages = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await?
        .json()?;
    assert_eq!(messages["schema_version"], 1);
    let id = messages["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["provider_id"] == "m-001")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let read = human(
        h.cli(&["mail", "read", "--account", &account, "--message", id])
            .await?,
    );
    assert!(read.contains("Attachments:"));
    assert!(read.contains("searchable"));
    let invalid_file = h.artifacts.join("invalid-action.json");
    std::fs::write(&invalid_file, b"untrusted-secret-value-not-for-output")?;
    let before = serde_json::to_value(h.google.control().snapshot().await)?;
    let invalid = h
        .cli(&[
            "mail",
            "change",
            "--account",
            &account,
            "--message",
            id,
            "--request-id",
            "715a730d-ec49-4f63-81ce-3a4d2ee21688",
            "--file",
            invalid_file.to_str().unwrap(),
        ])
        .await?;
    assert_eq!(invalid.status, 2);
    assert!(invalid.stdout.is_empty());
    let diagnostic = String::from_utf8(invalid.stderr)?;
    assert!(diagnostic.contains("mail change --help"));
    assert!(!diagnostic.contains("untrusted-secret"));
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await)?,
        before
    );
    assert!(human(
        h.cli(&["calendar", "refresh", "--account", &account, "--wait"])
            .await?
    )
    .contains("State: succeeded"));
    assert!(human(h.cli(&["calendar", "list", "--account", &account]).await?).contains("ID:"));
    human(
        h.cli(&[
            "calendar",
            "agenda",
            "--account",
            &account,
            "--from",
            "2026-03-01",
            "--to",
            "2026-04-01",
        ])
        .await?,
    );
    assert!(human(
        h.cli(&["mail", "draft", "list", "--account", &account])
            .await?
    )
    .contains("No items"));
    let input = h.artifacts.join("human-draft.json");
    std::fs::write(
        &input,
        serde_json::to_vec(
            &json!({"to":[{"address":"recipient@example.test"}],"subject":"Human output draft","text":"Offline human-mode acceptance"}),
        )?,
    )?;
    let draft = human(
        h.cli(&[
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            input.to_str().unwrap(),
        ])
        .await?,
    );
    assert!(draft.contains("Subject: Human output draft"));
    let drafts = h
        .cli(&["--json", "mail", "draft", "list", "--account", &account])
        .await?
        .json()?;
    let draft_id = drafts["result"]["items"][0]["id"].as_str().unwrap();
    assert!(human(
        h.cli(&[
            "mail",
            "draft",
            "show",
            "--account",
            &account,
            "--draft",
            draft_id
        ])
        .await?
    )
    .contains("Human output draft"));
    let sent = human(
        h.cli(&[
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            draft_id,
            "--request-id",
            "69706745-6c7f-4d56-a095-a7ea73870c14",
            "--wait",
        ])
        .await?,
    );
    assert!(sent.contains("State: applied"));
    assert_eq!(
        h.google.control().snapshot().await.mail["alpha@example.test"]
            .accepted_sends
            .len(),
        1
    );
    let operations = human(h.cli(&["operation", "list", "--account", &account]).await?);
    assert!(operations.contains("applied"));
    let missing = h
        .cli(&[
            "mail",
            "read",
            "--account",
            &account,
            "--message",
            "not-a-saved-message",
        ])
        .await?;
    assert_eq!(missing.status, 4);
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8(missing.stderr)?.contains("Error:"));
    h.shutdown().await?;
    Ok(())
}
