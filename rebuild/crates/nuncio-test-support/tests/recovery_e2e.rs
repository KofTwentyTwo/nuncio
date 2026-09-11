#![allow(clippy::unwrap_used)]
#[path = "support/reconciliation_mutations_e2e.rs"]
mod reconciliation_mutations_e2e;
use nuncio_test_support::{
    google::Seed,
    process::{CliOutput, E2eHarness},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

fn result(output: CliOutput) -> Value {
    assert_eq!(
        output.status,
        0,
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.json().unwrap()["result"].clone()
}
const SECRET: &[u8] = br#"{"passphrase":"synthetic recovery e2e passphrase canary"}"#;

#[tokio::test]
async fn actual_cli_backup_restore_preserves_pdf_and_held_request_until_explicit_new_submission() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let original_profile = h.directory.clone();
    let original_status = result(h.cli(&["--json", "system", "status"]).await.unwrap());
    let input = h.artifacts.join("draft.json");
    std::fs::write(&input,serde_json::to_vec(&json!({"to":[{"address":"recipient@example.test"}],"subject":"Recovery draft canary","text":"A durable draft and real PDF"})).unwrap()).unwrap();
    let saved = result(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            input.to_str().unwrap(),
        ])
        .await
        .unwrap(),
    );
    let draft = saved["id"].as_str().unwrap();
    let pdf = include_bytes!("../../../tests/fixtures/recovery/queued-draft.pdf");
    let pdf_path = h.artifacts.join("original-attachment.pdf");
    std::fs::write(&pdf_path, pdf).unwrap();
    let attached = result(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "attach",
            "--account",
            &account,
            "--draft",
            draft,
            "--version",
            "1",
            "--file",
            pdf_path.to_str().unwrap(),
            "--mime-type",
            "application/pdf",
        ])
        .await
        .unwrap(),
    );
    assert_eq!(attached["version"], 2);
    let barriers = h.artifacts.join("barriers");
    std::fs::write(barriers.join("operation_before_dispatch.arm"), []).unwrap();
    let send = [
        "--json",
        "mail",
        "send",
        "--account",
        &account,
        "--draft",
        draft,
        "--request-id",
        "1a9bd650-632a-4a10-82df-64a1df6d4be5",
        "--version",
        "2",
    ];
    let queued = result(h.cli(&send).await.unwrap());
    assert_eq!(queued["state"], "queued");
    let operation = queued["id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !barriers.join("operation_before_dispatch.entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    let output = h.artifacts.join("snapshot.nuncio");
    let created = result(
        h.cli_with_stdin(
            &[
                "--json",
                "backup",
                "create",
                "--output",
                output.to_str().unwrap(),
            ],
            SECRET,
        )
        .await
        .unwrap(),
    );
    let cipher = std::fs::read(&output).unwrap();
    assert!(!cipher.starts_with(b"SQLite format 3"));
    assert!(!cipher
        .windows(b"Recovery draft canary".len())
        .any(|v| v == b"Recovery draft canary"));
    assert_eq!(
        created["backup"]["sha256"],
        format!("{:x}", Sha256::digest(&cipher))
    );
    assert_eq!(created["backup"]["drafts"], 1);
    assert_eq!(created["backup"]["operations"], 1);
    let inspected = result(
        h.cli_with_stdin(
            &[
                "--json",
                "backup",
                "inspect",
                "--file",
                output.to_str().unwrap(),
            ],
            SECRET,
        )
        .await
        .unwrap(),
    );
    assert_eq!(inspected, created["backup"]);
    let wrong = h
        .cli_with_stdin(
            &[
                "--json",
                "backup",
                "inspect",
                "--file",
                output.to_str().unwrap(),
            ],
            br#"{"passphrase":"a wrong synthetic recovery phrase"}"#,
        )
        .await
        .unwrap();
    assert_eq!(wrong.status, 2);
    assert_eq!(std::fs::read(&output).unwrap(), cipher);
    let before_keys: Value =
        serde_json::from_slice(&std::fs::read(&h.secrets_file).unwrap()).unwrap();
    let restored = result(
        h.cli_with_stdin(
            &[
                "--json",
                "backup",
                "restore",
                "--file",
                output.to_str().unwrap(),
                "--new-profile",
                "recovered",
            ],
            SECRET,
        )
        .await
        .unwrap(),
    );
    assert_eq!(restored["held_operations"], 1);
    assert_eq!(restored["backup"], inspected);
    assert_ne!(restored["profile_id"], original_status["profile_id"]);
    let after_keys: Value =
        serde_json::from_slice(&std::fs::read(&h.secrets_file).unwrap()).unwrap();
    for (name, value) in before_keys.as_object().unwrap() {
        assert!(
            after_keys.get(name) == Some(value),
            "original key entry changed during restore"
        );
    }
    assert_eq!(std::fs::read(&output).unwrap(), cipher);
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.force_kill().await.unwrap();
    std::fs::remove_file(barriers.join("operation_before_dispatch.entered")).unwrap();
    std::fs::remove_file(pdf_path).unwrap();
    std::fs::remove_file(input).unwrap();
    h.directory = h.artifacts.join("recovered");
    h.restart().await.unwrap();
    let shown = result(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "show",
            "--account",
            &account,
            "--draft",
            draft,
        ])
        .await
        .unwrap(),
    );
    assert_eq!(shown, attached);
    let operation_args = [
        "--json",
        "operation",
        "show",
        "--account",
        &account,
        "--operation",
        operation,
    ];
    let held = result(h.cli(&operation_args).await.unwrap());
    assert_eq!(held["state"], "uncertain");
    assert_eq!(held["needs_reconciliation"], false);
    assert_eq!(held["request_id"], queued["request_id"]);
    assert!(
        held["updated_at_ms"].as_i64().unwrap()
            <= queued["updated_at_ms"].as_i64().unwrap() + 60000,
        "restore must use the injected engine clock"
    );
    assert_eq!(
        h.connect_google("alpha@example.test").await.unwrap(),
        account
    );
    result(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await
            .unwrap(),
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(result(h.cli(&operation_args).await.unwrap()), held);
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    let repeated = result(h.cli(&send).await.unwrap());
    assert_eq!(repeated["id"], operation);
    assert_eq!(repeated["state"], "uncertain");
    let mut held = held;
    for (request_id, resume) in [
        ("73e980ec-48a0-4229-9e45-ee05d9dfdd85", false),
        ("61f73148-e6b0-4261-acb9-b2bd7807f2f9", true),
    ] {
        let version = held["version"].as_u64().unwrap().to_string();
        let mut args = vec![
            "--json",
            "operation",
            "reconcile",
            "--account",
            &account,
            "--operation",
            operation,
            "--request-id",
            request_id,
            "--version",
            &version,
            "--wait",
        ];
        if resume {
            args.push("--resume-safe");
        }
        let outcome = h.cli(&args).await.unwrap();
        assert_eq!(
            outcome.status, 5,
            "negative send evidence must remain uncertain"
        );
        held = result(h.cli(&operation_args).await.unwrap());
        assert_eq!(held["state"], "uncertain");
        assert_eq!(held["needs_reconciliation"], false);
        assert_eq!(held["error_code"], "no_positive_send_evidence");
        assert_eq!(held["reconciliation"]["request_id"], request_id);
        assert!(h
            .google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .is_empty());
        h.force_kill().await.unwrap();
        h.restart().await.unwrap();
        assert_eq!(result(h.cli(&operation_args).await.unwrap()), held);
        assert_eq!(h.cli(&args).await.unwrap().status, 5);
        assert_eq!(result(h.cli(&operation_args).await.unwrap()), held);
    }
    let decision = h.artifacts.join("explicit-new-submission.json");
    std::fs::write(&decision,serde_json::to_vec(&json!({"decision":"resend","request_id":"9a706ba1-5a9c-4f5d-8eb2-7f99d089b9c7","reason":"Explicitly submit restored frozen content with duplicate risk acknowledged"})).unwrap()).unwrap();
    let version = held["version"].as_u64().unwrap().to_string();
    let mut resolve = vec![
        "--json",
        "operation",
        "resolve",
        "--account",
        &account,
        "--operation",
        operation,
        "--version",
        &version,
        "--file",
        decision.to_str().unwrap(),
    ];
    assert_eq!(h.cli(&resolve).await.unwrap().status, 2);
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    resolve.push("--accept-duplicate-risk");
    let resolved = result(h.cli(&resolve).await.unwrap());
    let replacement = resolved["resolutions"][0]["replacement_id"]
        .as_str()
        .unwrap();
    let accepted = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let accepted = h
                .google
                .control()
                .accepted_sends("alpha@example.test")
                .await;
            if !accepted.is_empty() {
                break accepted;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(accepted.len(), 1);
    let parsed = mailparse::parse_mail(&accepted[0].raw).unwrap();
    let attachment = parsed
        .subparts
        .iter()
        .find(|p| p.ctype.mimetype == "application/pdf")
        .unwrap();
    assert_eq!(attachment.get_body_raw().unwrap(), pdf);
    let completed = result(
        h.cli(&[
            "--json",
            "operation",
            "wait",
            "--account",
            &account,
            "--operation",
            replacement,
        ])
        .await
        .unwrap(),
    );
    assert_eq!(completed["state"], "applied");
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(result(h.cli(&resolve).await.unwrap()), resolved);
    assert_eq!(
        h.google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .len(),
        1
    );
    assert_eq!(result(h.cli(&send).await.unwrap())["id"], operation);
    assert!(original_profile.join("store.db").is_file());
    assert_eq!(std::fs::read(&output).unwrap(), cipher);
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn positive_restored_send_reconciliation_survives_lost_admission_ack_and_sigkill_before_receipt(
) {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    use nuncio_test_support::process::{binary, isolated_command};
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let original = h.directory.clone();
    let draft_file = h.artifacts.join("reconciliation-draft.json");
    std::fs::write(&draft_file,br#"{"to":[{"address":"recipient@example.test"}],"subject":"Recovered original send","text":"Immutable content across lost acknowledgements"}"#).unwrap();
    let draft = result(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            draft_file.to_str().unwrap(),
        ])
        .await
        .unwrap(),
    );
    let barriers = h.artifacts.join("barriers");
    std::fs::write(barriers.join("operation_before_dispatch.arm"), []).unwrap();
    let queued = result(
        h.cli(&[
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            draft["id"].as_str().unwrap(),
            "--request-id",
            "c4d5f2b2-bf3a-41f7-a6d2-ac5f75f684ce",
        ])
        .await
        .unwrap(),
    );
    async fn wait(path: &std::path::Path) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    wait(&barriers.join("operation_before_dispatch.entered")).await;
    let file = h.artifacts.join("queued.nuncio");
    result(
        h.cli_with_stdin(
            &[
                "--json",
                "backup",
                "create",
                "--output",
                file.to_str().unwrap(),
            ],
            SECRET,
        )
        .await
        .unwrap(),
    );
    h.google
        .control()
        .inject(Fault {
            method: "POST".into(),
            path: "/gmail/v1/users/me/messages/send".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::After,
            action: FaultAction::Disconnect,
        })
        .await;
    std::fs::write(barriers.join("operation_before_dispatch.release"), []).unwrap();
    let id = queued["id"].as_str().unwrap();
    result(
        h.cli(&[
            "--json",
            "operation",
            "wait",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap(),
    );
    assert_eq!(
        h.google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .len(),
        1
    );
    let restored = result(
        h.cli_with_stdin(
            &[
                "--json",
                "backup",
                "restore",
                "--file",
                file.to_str().unwrap(),
                "--new-profile",
                "reconciled",
            ],
            SECRET,
        )
        .await
        .unwrap(),
    );
    assert_eq!(restored["held_operations"], 1);
    h.shutdown().await.unwrap();
    h.directory = h.artifacts.join("reconciled");
    h.restart().await.unwrap();
    assert_eq!(
        h.connect_google("alpha@example.test").await.unwrap(),
        account
    );
    let show = [
        "--json",
        "operation",
        "show",
        "--account",
        &account,
        "--operation",
        id,
    ];
    let held = result(h.cli(&show).await.unwrap());
    let version = held["version"].as_u64().unwrap().to_string();
    let request_id = "836290f8-bbaa-46d8-af61-2d672b76c452";
    let args = [
        "--json",
        "operation",
        "reconcile",
        "--account",
        &account,
        "--operation",
        id,
        "--request-id",
        request_id,
        "--version",
        &version,
    ];
    for name in ["reconciliation_after_admission", "operation_before_receipt"] {
        for suffix in ["entered", "release"] {
            let path = barriers.join(format!("{name}.{suffix}"));
            if path.exists() {
                std::fs::remove_file(path).unwrap();
            }
        }
        std::fs::write(barriers.join(format!("{name}.arm")), []).unwrap();
    }
    let before = h.google.control().snapshot().await;
    let stdout = h.artifacts.join("lost-reconciliation.stdout.log");
    let mut cli = isolated_command(&binary("NUNCIO_E2E_CLI").unwrap(), &h.artifacts.join("tmp"))
        .arg("--endpoint")
        .arg(&h.endpoint)
        .arg("--data-dir")
        .arg(&h.directory)
        .arg("--test-secrets-file")
        .arg(&h.secrets_file)
        .args(args)
        .stdout(std::fs::File::create(&stdout).unwrap())
        .stderr(std::fs::File::create(h.artifacts.join("lost-reconciliation.stderr.log")).unwrap())
        .spawn()
        .unwrap();
    wait(&barriers.join("reconciliation_after_admission.entered")).await;
    wait(&barriers.join("operation_before_receipt.entered")).await;
    assert!(
        cli.try_wait().unwrap().is_none(),
        "CLI received an acknowledgement before its daemon checkpoint"
    );
    assert_eq!(
        h.google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .len(),
        1
    );
    h.force_kill().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), cli.wait())
            .await
            .unwrap()
            .unwrap()
            .code(),
        Some(4)
    );
    let failed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(stdout).unwrap()).unwrap();
    assert!(failed.get("result").is_none());
    assert_eq!(failed["error"]["code"], "request_outcome_unknown");
    assert_eq!(failed["error"]["retryable"], true);
    h.restart().await.unwrap();
    result(
        h.cli(&[
            "--json",
            "operation",
            "wait",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap(),
    );
    let observed = result(h.cli(&show).await.unwrap());
    assert_eq!(observed["id"], queued["id"]);
    assert_eq!(observed["request_id"], queued["request_id"]);
    assert_eq!(observed["reconciliation"]["request_id"], request_id);
    assert_eq!(observed["reconciliation"]["first_attempt_ordinal"], 1);
    assert_eq!(result(h.cli(&args).await.unwrap()), observed);
    let attempts = result(
        h.cli(&[
            "--json",
            "operation",
            "attempts",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap(),
    );
    let attempts = attempts["items"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["ordinal"], 2);
    assert_eq!(attempts[0]["kind"], "reconcile");
    assert_eq!(attempts[0]["outcome"], "applied");
    assert_eq!(attempts[0]["receipts"].as_array().unwrap().len(), 1);
    assert_eq!(attempts[0]["receipts"][0]["source"], "positive_read");
    assert_eq!(attempts[1]["outcome"], "uncertain");
    assert_eq!(attempts[1]["error_code"], "interrupted");
    assert_eq!(attempts[1]["receipts"], json!([]));
    let after = h.google.control().snapshot().await;
    assert_eq!(
        serde_json::to_value(after.mail).unwrap(),
        serde_json::to_value(before.mail).unwrap()
    );
    assert_eq!(
        serde_json::to_value(after.calendars).unwrap(),
        serde_json::to_value(before.calendars).unwrap()
    );
    assert_eq!(
        h.google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .len(),
        1
    );
    assert!(original.join("store.db").is_file());
    h.shutdown().await.unwrap();
}

#[path = "support/restore_crash_e2e.rs"]
mod restore_crash_e2e;
