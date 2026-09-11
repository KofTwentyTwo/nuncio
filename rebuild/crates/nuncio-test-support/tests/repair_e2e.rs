#![allow(clippy::unwrap_used)]
use nuncio_test_support::{google::Seed, process::E2eHarness, TestError};
use serde_json::{json, Value};
async fn cli(h: &E2eHarness, args: &[&str]) -> Value {
    let result = h.cli(args).await.unwrap();
    assert_eq!(
        result.status,
        0,
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.json().unwrap()["result"].clone()
}
fn arm(h: &E2eHarness, name: &str) {
    let dir = h.artifacts.join("barriers");
    for suffix in ["entered", "release"] {
        let file = dir.join(format!("{name}.{suffix}"));
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }
    std::fs::write(dir.join(format!("{name}.arm")), []).unwrap();
}
async fn wait(h: &E2eHarness, name: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !h
            .artifacts
            .join("barriers")
            .join(format!("{name}.entered"))
            .exists()
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unreadable_profile_refuses_daemon_and_repair_without_replacing_data_or_keys(
) -> Result<(), TestError> {
    use nuncio_test_support::process::{binary, isolated_command};
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let account = h.connect_google("alpha@example.test").await?;
    cli(&h, &["--json", "sync", "--account", &account, "--wait"]).await;
    let before = cli(&h, &["--json", "mail", "list", "--account", &account]).await;
    h.shutdown().await?;
    let path = h.directory.join("store.db");
    let original = std::fs::read(&path)?;
    let metadata = std::fs::read(h.directory.join("profile.json"))?;
    let keys = zeroize::Zeroizing::new(std::fs::read(&h.secrets_file)?);
    let mut corrupt = original.clone();
    corrupt[0] ^= 0xff;
    let requests = serde_json::to_value(h.google.control().snapshot().await.requests)?;
    for (label, damaged) in [
        ("corrupted-header", corrupt),
        ("truncated", original[..32].to_vec()),
    ] {
        std::fs::write(&path, &damaged)?;
        let ready = h.artifacts.join(format!("unreadable-{label}.ready.json"));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            isolated_command(&binary("NUNCIO_E2E_DAEMON")?, &h.artifacts.join("tmp"))
                .args(["--bind", "127.0.0.1:0", "--data-dir"])
                .arg(&h.directory)
                .arg("--test-secrets-file")
                .arg(&h.secrets_file)
                .arg("--test-config")
                .arg(h.artifacts.join("daemon-1.test.json"))
                .arg("--ready-file")
                .arg(&ready)
                .output(),
        )
        .await??;
        assert_eq!(result.status.code(), Some(1));
        assert!(!ready.exists());
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("Database key is incorrect or the database is damaged"));
        std::fs::write(
            h.artifacts.join(format!("unreadable-{label}.stderr.log")),
            &result.stderr,
        )?;
        for dry in [false, true] {
            let mut args = vec!["--json", "repair", "--account", &account, "--scope", "mail"];
            if dry {
                args.push("--dry-run");
            }
            let result = h.cli(&args).await?;
            assert_eq!(result.status, 4);
            assert_eq!(result.json()?["error"]["code"], "unavailable");
            assert!(
                std::fs::read(&path)? == damaged,
                "failed repair rewrote a damaged original"
            );
            assert!(
                std::fs::read(&h.secrets_file)? == *keys,
                "failed repair changed profile credentials"
            );
            assert_eq!(std::fs::read(h.directory.join("profile.json"))?, metadata);
            assert_eq!(
                serde_json::to_value(h.google.control().snapshot().await.requests)?,
                requests
            );
        }
    }
    // Only the test restores its retained synthetic original, while no daemon owns it.
    std::fs::write(&path, original)?;
    h.restart().await?;
    let after = cli(&h, &["--json", "mail", "list", "--account", &account]).await;
    assert_eq!(after["items"], before["items"]);
    assert_eq!(after["coverage"], before["coverage"]);
    h.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn actual_cli_repair_survives_crash_preserves_queued_mail_and_never_changes_remote_effects(
) -> Result<(), TestError> {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let account = h.connect_google("alpha@example.test").await?;
    cli(&h, &["--json", "sync", "--account", &account, "--wait"]).await;
    let editor = h.artifacts.join("repair-draft.json");
    std::fs::write(&editor,json!({"to":[{"address":"recipient@example.test"}],"subject":"Preserved through repair","text":"Local queued intent"}).to_string())?;
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
    arm(&h, "operation_before_dispatch");
    let send = [
        "--json",
        "mail",
        "send",
        "--account",
        &account,
        "--draft",
        draft_id,
        "--request-id",
        "4a18e69c-38ea-4818-a7c9-b18d765a7f1a",
    ];
    let queued = cli(&h, &send).await;
    let operation = queued["id"].as_str().unwrap();
    wait(&h, "operation_before_dispatch").await;
    let counts = serde_json::to_value(h.google.control().snapshot().await.requests)?;
    let preview = cli(
        &h,
        &[
            "--json",
            "repair",
            "--account",
            &account,
            "--scope",
            "mail",
            "--dry-run",
        ],
    )
    .await;
    assert_eq!(preview["projection"]["messages"], 3);
    assert_eq!(preview["preserved"]["operations"], 1);
    assert_eq!(preview["preserved"]["drafts"], 1);
    assert!(preview["run"].is_null());
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await.requests)?,
        counts
    );
    h.google
        .control()
        .delete_message("alpha@example.test", "m-003")
        .await?;
    let repaired = cli(
        &h,
        &[
            "--json",
            "repair",
            "--account",
            &account,
            "--scope",
            "mail",
            "--wait",
        ],
    )
    .await;
    assert_eq!(repaired["run"]["state"], "succeeded");
    assert_eq!(repaired["run"]["mode"], "full");
    let messages = cli(&h, &["--json", "mail", "list", "--account", &account]).await;
    assert_eq!(messages["items"].as_array().unwrap().len(), 2);
    let repair = [
        "--json",
        "repair",
        "--account",
        &account,
        "--scope",
        "calendar",
        "--from",
        "2026-03-01",
        "--to",
        "2026-03-20",
    ];
    let mut waiting = repair.to_vec();
    waiting.push("--wait");
    cli(&h, &waiting).await;
    let agenda = [
        "--json",
        "calendar",
        "agenda",
        "--account",
        &account,
        "--from",
        "2026-03-01",
        "--to",
        "2026-03-20",
    ];
    let before = cli(&h, &agenda).await;
    h.google.control().put_event("alpha@example.test","primary",json!({"id":"alldate01","summary":"External edit pending repair","status":"confirmed","start":{"date":"2026-03-10"},"end":{"date":"2026-03-11"}})).await?;
    let remote = h.google.control().snapshot().await;
    arm(&h, "calendar-repair-before-promotion");
    let pending = cli(&h, &repair).await;
    wait(&h, "calendar-repair-before-promotion").await;
    let unchanged = cli(&h, &agenda).await;
    assert_eq!(unchanged["items"], before["items"]);
    assert_eq!(unchanged["coverage"], before["coverage"]);
    h.force_kill().await?;
    arm(&h, "operation_before_dispatch");
    h.restart().await?;
    wait(&h, "operation_before_dispatch").await;
    let run_id = pending["run"]["id"].as_str().unwrap();
    let run = cli(
        &h,
        &[
            "--json",
            "system",
            "sync-status",
            "--account",
            &account,
            "--run",
            run_id,
        ],
    )
    .await;
    assert_eq!(run["state"], "failed");
    assert_eq!(run["error_code"], "interrupted");
    let unchanged = cli(&h, &agenda).await;
    assert_eq!(unchanged["items"], before["items"]);
    assert_eq!(unchanged["coverage"], before["coverage"]);
    let done = cli(&h, &waiting).await;
    assert_eq!(done["run"]["state"], "succeeded");
    let final_agenda = cli(&h, &agenda).await;
    assert_eq!(
        final_agenda["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["provider_id"] == "alldate01")
            .unwrap()["summary"],
        "External edit pending repair"
    );
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
    assert_eq!(
        cli(
            &h,
            &[
                "--json",
                "operation",
                "show",
                "--account",
                &account,
                "--operation",
                operation
            ]
        )
        .await,
        queued
    );
    assert_eq!(cli(&h, &send).await, queued);
    let after = h.google.control().snapshot().await;
    assert_eq!(
        serde_json::to_value(after.calendars)?,
        serde_json::to_value(remote.calendars)?
    );
    assert_eq!(
        serde_json::to_value(after.mail)?,
        serde_json::to_value(remote.mail)?
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.google.stop().await?;
    let offline = cli(
        &h,
        &[
            "--json",
            "repair",
            "--account",
            &account,
            "--scope",
            "mail",
            "--dry-run",
        ],
    )
    .await;
    assert!(offline["dry_run"].as_bool().unwrap());
    assert_eq!(offline["preserved"]["operations"], 1);
    h.shutdown().await?;
    Ok(())
}
