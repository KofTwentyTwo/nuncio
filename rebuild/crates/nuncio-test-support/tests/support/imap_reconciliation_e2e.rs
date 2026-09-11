use super::imap_read_e2e::{arm, list, wait};
use super::imap_transfer_e2e::connect_with_policy;
use super::*;
use crate::imap_effects::effects;
use serde_json::Value;

const SECRET: &[u8] = br#"{"passphrase":"synthetic IMAP write recovery phrase"}"#;
fn result(output: nuncio_test_support::process::CliOutput) -> Result<Value, TestError> {
    assert_eq!(
        output.status,
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.json()?["result"].clone())
}
async fn show(h: &E2eHarness, account: &str, id: &str) -> Result<Value, TestError> {
    result(
        h.cli(&[
            "--json",
            "operation",
            "show",
            "--account",
            account,
            "--operation",
            id,
        ])
        .await?,
    )
}
#[tokio::test]
async fn restored_imap_cli_resumes_proven_work_across_sigkill_without_repeating_store_or_copy(
) -> Result<(), TestError> {
    for transfer in [false, true] {
        let mut mock = MockMailPlus::start(&["MOVE"]).await?;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let account = connect_with_policy(&h, &mock, "client_append").await?;
        mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":2,"add":["\\Deleted"],"remove":[]})).await?;
        result(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await?,
        )?;
        let messages = list(&h, &account).await?;
        let id = messages["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["subject"] == "Independent MailPlus fixture 0")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let original_raw = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}),
            )
            .await?;
        let file = h.artifacts.join("intent.json");
        std::fs::write(
            &file,
            serde_json::to_vec(&if transfer {
                json!({"schema_version":1,"action":"archive"})
            } else {
                json!({"schema_version":1,"action":"read","read":true})
            })?,
        )?;
        let checkpoint = if transfer {
            "operation_after_imap_copy"
        } else {
            "operation_before_dispatch"
        };
        arm(&h, checkpoint)?;
        let queued = result(
            h.cli(&[
                "--json",
                "mail",
                "change",
                "--account",
                &account,
                "--message",
                id,
                "--request-id",
                "d4358a29-fb70-40b7-b1a5-99a0b2589983",
                "--file",
                file.to_str().unwrap(),
            ])
            .await?,
        )?;
        let operation = queued["id"].as_str().unwrap();
        wait(&h, checkpoint).await?;
        let backup = h.artifacts.join("pending.nuncio");
        result(
            h.cli_with_stdin(
                &[
                    "--json",
                    "backup",
                    "create",
                    "--output",
                    backup.to_str().unwrap(),
                ],
                SECRET,
            )
            .await?,
        )?;
        let cipher = std::fs::read(&backup)?;
        let restored = result(
            h.cli_with_stdin(
                &[
                    "--json",
                    "backup",
                    "restore",
                    "--file",
                    backup.to_str().unwrap(),
                    "--new-profile",
                    "recovered",
                ],
                SECRET,
            )
            .await?,
        )?;
        assert_eq!(restored["held_operations"], 1);
        let original = h.directory.clone();
        let before = effects(&mut mock).await?;
        h.force_kill().await?;
        h.directory = restored["directory"].as_str().unwrap().into();
        h.restart().await?;
        assert_eq!(
            connect_with_policy(&h, &mock, "client_append").await?,
            account
        );
        let held = show(&h, &account, operation).await?;
        assert_eq!(held["state"], "uncertain");
        let observe = h
            .cli(&[
                "--json",
                "operation",
                "reconcile",
                "--account",
                &account,
                "--operation",
                operation,
                "--request-id",
                "0c5c5d51-0556-426f-817c-a11fb770bd86",
                "--version",
                &held["version"].to_string(),
                "--wait",
            ])
            .await?;
        assert_eq!(observe.status, 5);
        let observed = show(&h, &account, operation).await?;
        assert_eq!(
            observed["error_code"],
            if transfer {
                "imap_source_removal_requires_resume"
            } else {
                "reconciliation_resume_required"
            }
        );
        assert_eq!(effects(&mut mock).await?, before);
        let version = observed["version"].to_string();
        let resume = [
            "--json",
            "operation",
            "reconcile",
            "--account",
            &account,
            "--operation",
            operation,
            "--request-id",
            "c5fa1ce6-7a47-48f8-9fef-a4b8c3a04dbe",
            "--version",
            &version,
            "--resume-safe",
        ];
        if transfer {
            arm(&h, "operation_before_receipt")?;
        } else {
            arm(&h, "operation_job_finished")?;
        }
        result(h.cli(&resume).await?)?;
        if !transfer {
            wait(&h, "operation_job_finished").await?;
            assert_eq!(show(&h, &account, operation).await?["state"], "retry_wait");
            assert_eq!(effects(&mut mock).await?, before);
            arm(&h, "operation_before_receipt")?;
            std::fs::write(
                h.artifacts.join("barriers/operation_job_finished.release"),
                [],
            )?;
        }
        wait(&h, "operation_before_receipt").await?;
        let accepted = effects(&mut mock).await?;
        assert_eq!(accepted["writes"]["imap UID STORE"], json!([1, 1]));
        assert_eq!(
            accepted["writes"]["imap UID COPY"],
            json!([u64::from(transfer), u64::from(transfer)])
        );
        assert_eq!(
            accepted["writes"]["imap UID EXPUNGE"],
            json!([u64::from(transfer), u64::from(transfer)])
        );
        for verb in ["imap UID MOVE", "imap EXPUNGE", "imap APPEND", "smtp DATA"] {
            assert_eq!(accepted["writes"][verb], json!([0, 0]));
        }
        h.force_kill().await?;
        h.restart().await?;
        result(
            h.cli(&[
                "--json",
                "operation",
                "wait",
                "--account",
                &account,
                "--operation",
                operation,
            ])
            .await?,
        )?;
        let finished = show(&h, &account, operation).await?;
        assert_eq!(finished["state"], "applied");
        assert_eq!(finished["request_id"], queued["request_id"]);
        assert_eq!(finished["desired_state_json"], queued["desired_state_json"]);
        assert_eq!(result(h.cli(&resume).await?)?, finished);
        assert_eq!(effects(&mut mock).await?, accepted);
        assert_eq!(accepted["other"], before["other"]);
        assert_eq!(accepted["deliveries"], before["deliveries"]);
        let inbox = accepted["mailboxes"]["INBOX"]["messages"]
            .as_array()
            .unwrap();
        let unrelated = inbox.iter().find(|m| m["uid"] == 2).unwrap();
        assert_eq!(
            unrelated,
            before["mailboxes"]["INBOX"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["uid"] == 2)
                .unwrap()
        );
        assert!(unrelated["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("\\Deleted")));
        assert_eq!(inbox.len(), if transfer { 1 } else { 2 });
        if !transfer {
            assert!(inbox.iter().find(|m| m["uid"] == 1).unwrap()["flags"]
                .as_array()
                .unwrap()
                .contains(&json!("\\Seen")));
        }
        let local = list(&h, &account).await?;
        let message = local["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["subject"] == "Independent MailPlus fixture 0")
            .unwrap();
        assert_eq!(
            message["collections"][0]["name"],
            if transfer { "Archive" } else { "INBOX" }
        );
        let download = h.artifacts.join("recovered.eml");
        result(
            h.cli(&[
                "--json",
                "mail",
                "raw",
                "--account",
                &account,
                "--message",
                message["id"].as_str().unwrap(),
                "--output",
                download.to_str().unwrap(),
            ])
            .await?,
        )?;
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        assert_eq!(
            std::fs::read(download)?,
            STANDARD.decode(original_raw["raw_base64"].as_str().unwrap())?
        );
        let history = result(
            h.cli(&[
                "--json",
                "operation",
                "attempts",
                "--account",
                &account,
                "--operation",
                operation,
            ])
            .await?,
        )?;
        let attempts = history["items"].as_array().unwrap();
        assert_eq!(attempts.len(), 4);
        assert_eq!(
            attempts
                .iter()
                .map(|a| a["ordinal"].as_u64().unwrap())
                .collect::<std::collections::BTreeSet<_>>(),
            (1..=4).collect()
        );
        assert_eq!(
            attempts.iter().filter(|a| a["kind"] == "dispatch").count(),
            1
        );
        assert!(
            attempts.iter().find(|a| a["ordinal"] == 3).unwrap()["receipts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            attempts
                .iter()
                .flat_map(|a| a["receipts"].as_array().unwrap())
                .filter(|r| r["kind"] == "imap_copy")
                .count(),
            usize::from(transfer)
        );
        assert_eq!(effects(&mut mock).await?, accepted);
        assert_eq!(std::fs::read(&backup)?, cipher);
        assert!(original.join("store.db").is_file());
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
