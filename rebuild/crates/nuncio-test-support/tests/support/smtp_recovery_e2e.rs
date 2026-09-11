use super::*;
use super::{
    imap_read_e2e::{arm, wait},
    imap_transfer_e2e::connect_with_policy,
    smtp_e2e::settled,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mailparse::MailHeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SECRET: &[u8] = br#"{"passphrase":"synthetic MailPlus recovery test phrase"}"#;
const PDF: &[u8] = include_bytes!("../../../../tests/fixtures/recovery/queued-draft.pdf");

fn value(output: nuncio_test_support::process::CliOutput) -> Result<Value, TestError> {
    assert_eq!(
        output.status,
        0,
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.json()?["result"].clone())
}

async fn observe(
    mock: &mut MockMailPlus,
    server_sent: bool,
    submissions: u64,
) -> Result<Vec<String>, TestError> {
    if server_sent {
        mock.control(json!({"command":"wait_server_sent","count":submissions}))
            .await?;
    }
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["requests"]["smtp DATA"], submissions);
    assert_eq!(snapshot["accepted"]["smtp DATA"], submissions);
    let copies = if server_sent { 0 } else { submissions - 1 };
    for kind in ["requests", "accepted"] {
        assert_eq!(snapshot[kind]["imap APPEND"].as_u64().unwrap_or(0), copies);
    }
    let envelopes = snapshot["smtp_deliveries"].as_array().unwrap();
    assert_eq!(envelopes.len(), submissions as usize);
    let smtp = mock.control(json!({"command":"smtp"})).await?;
    assert_eq!(smtp["total"], submissions);
    let mut ids = Vec::new();
    for message in smtp["messages"].as_array().unwrap() {
        assert_eq!(message["Username"], "alpha@example.test");
        let raw = mock
            .control(json!({"command":"smtp_raw","transport":"starttls","id":message["ID"]}))
            .await?;
        let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
        let envelope = envelopes
            .iter()
            .find(|e| {
                let n = e["wire_size"].as_u64().unwrap() as usize;
                raw.len() >= n
                    && format!("{:x}", Sha256::digest(&raw[raw.len() - n..])) == e["wire_sha256"]
            })
            .unwrap();
        assert_eq!(envelope["sender"], "alpha@example.test");
        assert_eq!(envelope["recipients"], json!(["beta@example.test"]));
        let wire = &raw[raw.len() - envelope["wire_size"].as_u64().unwrap() as usize..];
        let parsed = mailparse::parse_mail(wire)?;
        assert_eq!(
            parsed.headers.get_first_value("Subject").as_deref(),
            Some("MailPlus snapshot recovery")
        );
        assert_eq!(
            parsed
                .subparts
                .iter()
                .find(|p| p.ctype.mimetype == "application/pdf")
                .unwrap()
                .get_body_raw()?,
            PDF
        );
        ids.push(parsed.headers.get_first_value("Message-ID").unwrap());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), submissions as usize);
    let sent = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
        .await?;
    assert_eq!(
        sent["messages"].as_array().unwrap().len() as u64,
        if server_sent { submissions } else { copies }
    );
    for message in sent["messages"].as_array().unwrap() {
        let raw=mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":message["uid"]})).await?;
        let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
        assert_eq!(format!("{:x}", Sha256::digest(&raw)), message["sha256"]);
        let parsed = mailparse::parse_mail(&raw)?;
        assert!(ids.contains(&parsed.headers.get_first_value("Message-ID").unwrap()));
        assert_eq!(
            parsed
                .subparts
                .iter()
                .find(|p| p.ctype.mimetype == "application/pdf")
                .unwrap()
                .get_body_raw()?,
            PDF
        );
    }
    Ok(ids)
}

#[tokio::test]
async fn restore_of_queued_smtp_snapshot_never_repeats_post_snapshot_delivery_or_sent_copy(
) -> Result<(), TestError> {
    for server_sent in [false, true] {
        let mut mock = MockMailPlus::start_with_sent_policy(&[], server_sent).await?;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let outcome = exercise(&mut h, &mut mock, server_sent).await;
        // Stop independent children even when a recoverable test error occurs.
        let engine_stop = h.shutdown().await;
        let mock_stop = mock.shutdown().await;
        outcome?;
        engine_stop?;
        mock_stop?;
    }
    Ok(())
}
async fn exercise(
    h: &mut E2eHarness,
    mock: &mut MockMailPlus,
    server_sent: bool,
) -> Result<(), TestError> {
    let policy = if server_sent {
        "server"
    } else {
        "client_append"
    };
    let account = connect_with_policy(h, mock, policy).await?;
    value(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?,
    )?;
    let original = h.directory.clone();
    let config = value(
        h.cli(&["--json", "account", "imap-config", "--account", &account])
            .await?,
    )?;
    let editor = h.artifacts.join("recovery-draft.json");
    std::fs::write(&editor,json!({"to":[{"address":"beta@example.test"}],"subject":"MailPlus snapshot recovery","text":"Frozen PDF survives a stale snapshot"}).to_string())?;
    let draft = value(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            editor.to_str().unwrap(),
        ])
        .await?,
    )?;
    let draft = draft["id"].as_str().unwrap();
    let pdf = h.artifacts.join("recovery.pdf");
    std::fs::write(&pdf, PDF)?;
    let attached = value(
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
            pdf.to_str().unwrap(),
            "--mime-type",
            "application/pdf",
        ])
        .await?,
    )?;
    arm(h, "operation_before_dispatch")?;
    let send = [
        "--json",
        "mail",
        "send",
        "--account",
        &account,
        "--draft",
        draft,
        "--version",
        "2",
        "--request-id",
        "bcf8f2d1-0d6f-4c87-9d6d-3f4f28a11fa1",
    ];
    let queued = value(h.cli(&send).await?)?;
    let operation = queued["id"].as_str().unwrap();
    wait(h, "operation_before_dispatch").await?;
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    let backup = h.artifacts.join("smtp-snapshot.nuncio");
    let created = value(
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
    let ciphertext = std::fs::read(&backup)?;
    assert_eq!(created["backup"]["operations"], 1);
    assert_eq!(
        created["backup"]["sha256"],
        format!("{:x}", Sha256::digest(&ciphertext))
    );
    // The snapshot says queued, but the source really sends after it is taken.
    mock.control(json!({"command":"inject","name":"accepted-after-backup","protocol":"smtp","verb":"DATA","phase":"after","action":"disconnect"})).await?;
    std::fs::write(
        h.artifacts
            .join("barriers/operation_before_dispatch.release"),
        [],
    )?;
    mock.control(json!({"command":"wait_fault","name":"accepted-after-backup"}))
        .await?;
    let original_ids = observe(mock, server_sent, 1).await?;
    assert_eq!(
        settled(h, &account, operation, "uncertain").await?["error_code"],
        "smtp_acceptance_unknown"
    );
    let restored = value(
        h.cli_with_stdin(
            &[
                "--json",
                "backup",
                "restore",
                "--file",
                backup.to_str().unwrap(),
                "--new-profile",
                "smtp-recovered",
            ],
            SECRET,
        )
        .await?,
    )?;
    assert_eq!(restored["held_operations"], 1);
    assert_eq!(restored["backup"], created["backup"]);
    h.force_kill().await?;
    std::fs::remove_file(&editor)?;
    std::fs::remove_file(&pdf)?;
    h.directory = h.artifacts.join("smtp-recovered");
    h.restart().await?;
    assert_eq!(
        value(
            h.cli(&["--json", "account", "imap-config", "--account", &account])
                .await?
        )?,
        config
    );
    assert_eq!(
        value(
            h.cli(&[
                "--json",
                "mail",
                "draft",
                "show",
                "--account",
                &account,
                "--draft",
                draft
            ])
            .await?
        )?,
        attached
    );
    let held = settled(h, &account, operation, "uncertain").await?;
    assert_eq!(held["error_code"], "restored_snapshot_unreconciled");
    assert_eq!(held["request_id"], queued["request_id"]);
    assert_eq!(
        held["version"].as_u64().unwrap(),
        queued["version"].as_u64().unwrap() + 1
    );
    assert_eq!(connect_with_policy(h, mock, policy).await?, account);
    value(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?,
    )?;
    h.force_kill().await?;
    h.restart().await?;
    assert_eq!(settled(h, &account, operation, "uncertain").await?, held);
    assert_eq!(value(h.cli(&send).await?)?["id"], operation);
    assert_eq!(observe(mock, server_sent, 1).await?, original_ids);
    let attempts = value(
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
    assert_eq!(
        attempts["items"],
        json!([]),
        "post-snapshot receipts must never be invented"
    );
    let mut held = held;
    for (request_id, resume) in [
        ("9541df3e-4e3a-46db-9776-e380c6d9fafc", false),
        ("b5fc5848-ee70-42ee-b376-2b463dbfe7db", true),
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
        assert_eq!(h.cli(&args).await?.status, 5);
        held = settled(h, &account, operation, "uncertain").await?;
        assert_eq!(held["error_code"], "restored_smtp_acceptance_unknown");
        assert_eq!(held["reconciliation"]["request_id"], request_id);
        assert_eq!(observe(mock, server_sent, 1).await?, original_ids);
        h.force_kill().await?;
        h.restart().await?;
        assert_eq!(h.cli(&args).await?.status, 5);
        assert_eq!(settled(h, &account, operation, "uncertain").await?, held);
        assert_eq!(observe(mock, server_sent, 1).await?, original_ids);
        let attempts = value(
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
        for attempt in attempts["items"].as_array().unwrap() {
            assert_eq!(attempt["kind"], "reconcile");
            assert_eq!(attempt["outcome"], "uncertain");
            assert_eq!(attempt["receipts"], json!([]));
        }
    }
    let decision = h.artifacts.join("smtp-recovery-resend.json");
    std::fs::write(&decision,json!({"decision":"resend","request_id":"3aac3caa-a7a5-48aa-bdb6-268f3e8648d1","reason":"Explicitly accept duplicate risk after stale backup recovery"}).to_string())?;
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
    assert_eq!(h.cli(&resolve).await?.status, 2);
    assert_eq!(observe(mock, server_sent, 1).await?, original_ids);
    resolve.push("--accept-duplicate-risk");
    let resolved = value(h.cli(&resolve).await?)?;
    let replacement = resolved["resolutions"][0]["replacement_id"]
        .as_str()
        .unwrap();
    assert_ne!(replacement, operation);
    settled(h, &account, replacement, "applied").await?;
    let new_ids = observe(mock, server_sent, 2).await?;
    assert!(new_ids.contains(&original_ids[0]));
    h.force_kill().await?;
    h.restart().await?;
    assert_eq!(value(h.cli(&resolve).await?)?, resolved);
    assert_eq!(value(h.cli(&send).await?)?["id"], operation);
    assert_eq!(observe(mock, server_sent, 2).await?, new_ids);
    assert_eq!(std::fs::read(&backup)?, ciphertext);
    assert!(original.join("store.db").is_file());
    Ok(())
}
