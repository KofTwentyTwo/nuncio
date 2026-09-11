use super::{E2eHarness, Seed};
use mailparse::MailHeaderMap;
use std::time::Duration;

async fn wait_checkpoint(h: &E2eHarness, name: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while !h
        .artifacts
        .join("barriers")
        .join(format!("{name}.entered"))
        .exists()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker did not reach {name}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
fn arm(h: &E2eHarness, name: &str) {
    std::fs::write(h.artifacts.join("barriers").join(format!("{name}.arm")), []).unwrap();
}
fn release(h: &E2eHarness, name: &str) {
    std::fs::write(
        h.artifacts.join("barriers").join(format!("{name}.release")),
        [],
    )
    .unwrap();
}
async fn save(h: &E2eHarness, account: &str) -> (String, std::path::PathBuf) {
    let file = h.artifacts.join("crash-draft.json");
    std::fs::write(&file,br#"{"to":[{"address":"recipient@example.test"}],"subject":"Commit boundary","text":"Original frozen body"}"#).unwrap();
    let saved = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            account,
            "--file",
            file.to_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(saved.status, 0);
    (
        saved.json().unwrap()["result"]["id"]
            .as_str()
            .unwrap()
            .to_owned(),
        file,
    )
}

#[tokio::test]
async fn queued_send_survives_full_projection_reset_and_cancellation_wins_before_dispatch() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await
            .unwrap()
            .status,
        0
    );
    let messages = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let source = messages["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["provider_id"] == "m-001")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let prepared = h
        .cli(&[
            "--json",
            "mail",
            "forward",
            "--account",
            &account,
            "--message",
            source,
            "--to",
            "recipient@example.test",
        ])
        .await
        .unwrap();
    assert_eq!(prepared.status, 0);
    let prepared = prepared.json().unwrap()["result"].clone();
    assert!(!prepared["attachments"].as_array().unwrap().is_empty());
    let draft = prepared["id"].as_str().unwrap().to_owned();
    arm(&h, "operation_before_dispatch");
    arm(&h, "operation_job_finished");
    let queued = h
        .cli(&[
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "314c752c-70c7-4101-a5d3-030e1488ac25",
        ])
        .await
        .unwrap();
    assert_eq!(queued.status, 0);
    let op = queued.json().unwrap()["result"].clone();
    let id = op["id"].as_str().unwrap();
    wait_checkpoint(&h, "operation_before_dispatch").await;
    h.google
        .control()
        .delete_message("alpha@example.test", "m-001")
        .await
        .unwrap();
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &account, "--full", "--wait"])
            .await
            .unwrap()
            .status,
        0
    );
    let mail = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert!(mail["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["provider_id"] != "m-001"));
    let after = h
        .cli(&[
            "--json",
            "operation",
            "show",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        after["result"], op,
        "projection replacement cannot alter pending intent"
    );
    let retained = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "show",
            "--account",
            &account,
            "--draft",
            &draft,
        ])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        retained["result"], prepared,
        "a forward draft and its original attachments survive deletion of the source message"
    );
    let cancelled = h
        .cli(&[
            "--json",
            "operation",
            "cancel",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap();
    assert_eq!(cancelled.status, 0);
    assert_eq!(cancelled.json().unwrap()["result"]["state"], "cancelled");
    release(&h, "operation_before_dispatch");
    wait_checkpoint(&h, "operation_job_finished").await;
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    release(&h, "operation_job_finished");
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let after = h
        .cli(&[
            "--json",
            "operation",
            "wait",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap();
    assert_eq!(after.status, 5);
    assert_eq!(
        after.json().unwrap()["error"]["operation"]["state"],
        "cancelled"
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn send_commit_boundaries_and_concurrent_draft_edits_keep_receipts_and_remote_effects_consistent(
) {
    for (checkpoint, crash, expected_count) in [
        ("operation_after_attempt", true, 0),
        ("operation_before_receipt", true, 1),
        ("operation_after_attempt", false, 1),
    ] {
        let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
        let account = h.connect_google("alpha@example.test").await.unwrap();
        let (draft, file) = save(&h, &account).await;
        arm(&h, checkpoint);
        let command = [
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "8758e8d4-c95c-4b22-bf8c-881715a00e98",
            "--version",
            "1",
        ];
        let queued = h.cli(&command).await.unwrap();
        assert_eq!(queued.status, 0);
        let id = queued.json().unwrap()["result"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        wait_checkpoint(&h, checkpoint).await;
        let cancelled = h
            .cli(&[
                "--json",
                "operation",
                "cancel",
                "--account",
                &account,
                "--operation",
                &id,
            ])
            .await
            .unwrap();
        assert_eq!(
            cancelled.status, 5,
            "running send cannot be safely cancelled"
        );
        std::fs::write(&file,br#"{"to":[{"address":"different@example.test"}],"subject":"Changed draft","text":"Later body"}"#).unwrap();
        assert_eq!(
            h.cli(&[
                "--json",
                "mail",
                "draft",
                "save",
                "--account",
                &account,
                "--draft",
                &draft,
                "--version",
                "1",
                "--file",
                file.to_str().unwrap()
            ])
            .await
            .unwrap()
            .status,
            0
        );
        if crash {
            h.force_kill().await.unwrap();
            h.restart().await.unwrap();
        } else {
            release(&h, checkpoint);
        }
        let mut wait = command.to_vec();
        wait.push("--wait");
        let result = h.cli(&wait).await.unwrap();
        assert_eq!(result.status, if expected_count == 0 { 5 } else { 0 });
        let sends = h
            .google
            .control()
            .accepted_sends("alpha@example.test")
            .await;
        assert_eq!(
            sends.len(),
            expected_count,
            "a lost journal receipt never authorizes another send"
        );
        if let Some(sent) = sends.first() {
            let parsed = mailparse::parse_mail(&sent.raw).unwrap();
            assert_eq!(
                parsed.headers.get_first_value("Subject").as_deref(),
                Some("Commit boundary")
            );
            assert_eq!(
                parsed.get_body().unwrap().trim_end(),
                "Original frozen body"
            );
            assert_eq!(
                mailparse::addrparse(&parsed.headers.get_first_value("To").unwrap())
                    .unwrap()
                    .extract_single_info()
                    .unwrap()
                    .addr,
                "recipient@example.test"
            );
        } else {
            assert_eq!(
                result.json().unwrap()["error"]["operation"]["state"],
                "uncertain"
            );
        }
        assert!(h
            .google
            .control()
            .accepted_sends("beta@example.test")
            .await
            .is_empty());
        h.shutdown().await.unwrap();
    }
}
