use super::*;
use super::{
    imap_read_e2e::{arm, list, wait},
    imap_transfer_e2e::connect,
};
use serde_json::Value;

async fn cli(h: &E2eHarness, args: &[&str]) -> Value {
    let output = h.cli(args).await.unwrap();
    assert_eq!(
        output.status,
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.json().unwrap()["result"].clone()
}
async fn remote(mock: &mut MockMailPlus) -> Result<Value, TestError> {
    let mut result = serde_json::Map::new();
    for account in ["alpha@example.test", "beta@example.test"] {
        for mailbox in ["INBOX", "Archive", "Sent", "Trash"] {
            result.insert(
                format!("{account}/{mailbox}"),
                mock.control(json!({"command":"mailbox","account":account,"mailbox":mailbox}))
                    .await?,
            );
        }
    }
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    for kind in ["requests", "accepted"] {
        for verb in [
            "smtp DATA",
            "imap APPEND",
            "imap UID COPY",
            "imap UID MOVE",
            "imap UID STORE",
            "imap UID EXPUNGE",
            "imap EXPUNGE",
        ] {
            assert_eq!(
                snapshot[kind][verb].as_u64().unwrap_or(0),
                0,
                "repair unexpectedly performed {verb}"
            );
        }
    }
    assert_eq!(snapshot["smtp_deliveries"], json!([]));
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    Ok(Value::Object(result))
}

#[tokio::test]
async fn repair_cli_preserves_queued_smtp_and_remote_mail_through_cancel_and_sigkill(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let account = connect(&h, &mock).await?;
    let repair = ["--json", "repair", "--account", &account, "--scope", "mail"];
    let mut waiting = repair.to_vec();
    waiting.push("--wait");
    assert_eq!(cli(&h, &waiting).await["run"]["state"], "succeeded");
    let before = list(&h, &account).await?;
    let editor = h.artifacts.join("imap-repair-draft.json");
    std::fs::write(&editor, json!({"to":[{"address":"recipient@example.test"}],"subject":"Queued during repair","text":"Keep durable SMTP intent"}).to_string())?;
    let draft = cli(
        &h,
        &[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            editor.to_str().unwrap(),
        ],
    )
    .await;
    let draft_id = draft["id"].as_str().unwrap();
    arm(&h, "operation_before_dispatch")?;
    let send = [
        "--json",
        "mail",
        "send",
        "--account",
        &account,
        "--draft",
        draft_id,
        "--request-id",
        "bfed0a65-183e-4f18-8973-3c4813048207",
    ];
    let queued = cli(&h, &send).await;
    wait(&h, "operation_before_dispatch").await?;
    let mut dry = repair.to_vec();
    dry.push("--dry-run");
    let requests = mock.control(json!({"command":"snapshot"})).await?["requests"].clone();
    let preview = cli(&h, &dry).await;
    assert_eq!(preview["projection"]["messages"], 2);
    assert_eq!(preview["projection"]["search_entries"], 2);
    assert_eq!(preview["preserved"]["drafts"], 1);
    assert_eq!(preview["preserved"]["operations"], 1);
    assert_eq!(preview["preserved"]["attempts"], 0);
    assert!(preview["run"].is_null());
    assert_eq!(
        mock.control(json!({"command":"snapshot"})).await?["requests"],
        requests
    );
    mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"INBOX","uid":1,"destination":"Archive"})).await?;
    mock.control(
        json!({"command":"expunge","account":"alpha@example.test","mailbox":"INBOX","uid":2}),
    )
    .await?;
    let remote_before = remote(&mut mock).await?;
    for kill in [false, true] {
        arm(&h, "imap-before-promotion")?;
        let started = cli(&h, &repair).await;
        let run = started["run"]["id"].as_str().unwrap();
        wait(&h, "imap-before-promotion").await?;
        let during = list(&h, &account).await?;
        assert_eq!(during["items"], before["items"]);
        assert_eq!(during["coverage"], before["coverage"]);
        if kill {
            h.force_kill().await?;
            arm(&h, "operation_before_dispatch")?;
            h.restart().await?;
            wait(&h, "operation_before_dispatch").await?;
        } else {
            let cancelled = cli(
                &h,
                &[
                    "--json",
                    "system",
                    "cancel-sync",
                    "--account",
                    &account,
                    "--run",
                    run,
                ],
            )
            .await;
            assert_eq!(cancelled["state"], "cancelled");
        }
        let status = cli(
            &h,
            &[
                "--json",
                "system",
                "sync-status",
                "--account",
                &account,
                "--run",
                run,
            ],
        )
        .await;
        assert_eq!(status["state"], if kill { "failed" } else { "cancelled" });
        assert_eq!(
            status["error_code"],
            if kill { "interrupted" } else { "cancelled" }
        );
        let after = list(&h, &account).await?;
        assert_eq!(after["items"], before["items"]);
        assert_eq!(after["coverage"], before["coverage"]);
        assert_eq!(remote(&mut mock).await?, remote_before);
    }
    assert_eq!(cli(&h, &waiting).await["run"]["state"], "succeeded");
    let after = list(&h, &account).await?;
    assert_eq!(after["items"].as_array().unwrap().len(), 2);
    assert!(after["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["subject"] == "Independent MailPlus fixture 0"));
    assert!(after["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["collections"][0]["name"] == "Archive"));
    assert_eq!(
        cli(
            &h,
            &[
                "--json",
                "mail",
                "draft",
                "show",
                "--account",
                &account,
                "--draft",
                draft_id
            ]
        )
        .await,
        draft
    );
    assert_eq!(cli(&h, &send).await, queued);
    assert_eq!(remote(&mut mock).await?, remote_before);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
