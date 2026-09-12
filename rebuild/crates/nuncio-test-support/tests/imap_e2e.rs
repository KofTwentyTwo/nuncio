#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "support/imap_effects.rs"]
mod imap_effects;
#[path = "support/imap_reconciliation_e2e.rs"]
mod imap_reconciliation_e2e;
use nuncio_test_support::{google::Seed, imap::MockMailPlus, process::E2eHarness, TestError};
use serde_json::json;
#[path = "support/imap_flags_e2e.rs"]
mod imap_flags_e2e;
#[path = "support/imap_folder_e2e.rs"]
mod imap_folder_e2e;
#[path = "support/imap_read_e2e.rs"]
mod imap_read_e2e;
#[path = "support/imap_repair_e2e.rs"]
mod imap_repair_e2e;
#[path = "support/imap_transfer_e2e.rs"]
mod imap_transfer_e2e;
#[path = "support/imap_trash_e2e.rs"]
mod imap_trash_e2e;
#[path = "support/smtp_capability_e2e.rs"]
mod smtp_capability_e2e;
#[path = "support/smtp_confirmation_e2e.rs"]
mod smtp_confirmation_e2e;
#[path = "support/smtp_e2e.rs"]
mod smtp_e2e;
#[path = "support/smtp_recovery_e2e.rs"]
mod smtp_recovery_e2e;
#[path = "support/smtp_resend_e2e.rs"]
mod smtp_resend_e2e;
#[path = "support/smtp_resolution_evidence.rs"]
mod smtp_resolution_evidence;
#[path = "support/smtp_server_e2e.rs"]
mod smtp_server_e2e;

#[tokio::test]
async fn actual_cli_mailplus_reads_and_writes_survive_crashes() -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut harness = E2eHarness::start(Seed::TwoAccounts).await?;
    let public = json!({"schema_version":1,"address":"alpha@example.test","imap":{"host":"127.0.0.1","port":mock.ready.ports.imaps,"tls":"implicit","username":"alpha@example.test"},"smtp":{"host":"127.0.0.1","port":mock.ready.ports.smtp,"tls":"start_tls","username":"alpha@example.test"},"sent_policy":"client_append","sent_folder":"Sent","archive_folder":"Archive","trash_folder":"Trash","trusted_ca_pem":tokio::fs::read_to_string(&mock.ready.ca_file).await?});
    let path = harness.artifacts.join("imap-public.json");
    std::fs::write(&path, public.to_string())?;
    let password = mock.credential("alpha@example.test")?;
    let secret = zeroize::Zeroizing::new(
        json!({"imap_password":password,"smtp_password":password}).to_string(),
    );
    let output = harness
        .cli_with_stdin(
            &[
                "--json",
                "account",
                "connect-imap",
                "--config",
                path.to_str().unwrap(),
                "--credentials-stdin",
            ],
            secret.as_bytes(),
        )
        .await?;
    assert_eq!(
        output.status,
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = output.json()?;
    let account = result["result"]["account"]["id"]
        .as_str()
        .ok_or("missing account")?
        .to_owned();
    assert_eq!(result["result"]["account"]["provider"], "imap");
    assert_eq!(result["result"]["capabilities"]["move_messages"], true);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&password));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&password));
    harness.force_kill().await?;
    harness.restart().await?;
    let check = harness
        .cli(&["--json", "account", "check", "--account", &account])
        .await?;
    assert_eq!(check.status, 0);
    let read = harness
        .cli(&["--json", "account", "imap-config", "--account", &account])
        .await?;
    assert_eq!(read.status, 0);
    assert_eq!(read.json()?["result"]["config"], public);
    assert!(!String::from_utf8_lossy(&read.stdout).contains(&password));
    imap_read_e2e::reads(&mut harness, &mut mock, &account).await?;
    let metrics = harness.cli(&["--json", "system", "status"]).await?;
    assert_eq!(metrics.status, 0);
    let metrics = metrics.json()?["result"]["resources"].clone();
    assert_eq!(metrics["requests_active"], 0);
    assert_eq!(metrics["requests_peak"], 1);
    assert!(metrics["bytes_received"].as_u64().unwrap() > 0);
    assert!(metrics["storage_page_batches"].as_u64().unwrap() > 0);
    std::fs::write(
        harness.artifacts.join("imap-resources.json"),
        serde_json::to_vec_pretty(&metrics)?,
    )?;

    imap_flags_e2e::flags(&mut harness, &mut mock, &account).await?;
    imap_transfer_e2e::archive(&mut harness, &mut mock, &account).await?;
    let mut bad = public.clone();
    bad["password"] = json!("synthetic-canary-not-a-password");
    std::fs::write(&path, bad.to_string())?;
    let rejected = harness
        .cli_with_stdin(
            &[
                "--json",
                "account",
                "connect-imap",
                "--config",
                path.to_str().unwrap(),
                "--credentials-stdin",
            ],
            secret.as_bytes(),
        )
        .await?;
    assert_eq!(rejected.status, 2);
    assert!(!String::from_utf8_lossy(&rejected.stdout).contains("synthetic-canary"));
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    assert_eq!(
        mock.control(json!({"command":"snapshot"})).await?["smtp_deliveries"],
        json!([])
    );
    harness.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[path = "support/smtp_sent_reconciliation_e2e.rs"]
mod smtp_sent_reconciliation_e2e;
