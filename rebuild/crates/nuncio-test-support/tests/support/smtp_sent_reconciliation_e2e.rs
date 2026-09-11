use super::{
    imap_read_e2e::{arm, list, wait},
    imap_transfer_e2e::connect_with_policy,
    smtp_e2e::settled,
    *,
};
use crate::imap_effects::effects;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mailparse::MailHeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};
const SECRET: &[u8] = br#"{"passphrase":"synthetic accepted SMTP recovery phrase"}"#;
const PDF: &[u8] = include_bytes!("../../../../tests/fixtures/recovery/queued-draft.pdf");
fn value(output: nuncio_test_support::process::CliOutput) -> Result<Value, TestError> {
    assert_eq!(
        output.status,
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.json()?["result"].clone())
}
#[tokio::test]
async fn restored_accepted_smtp_cli_survives_sigkill_before_sent_observation_commit(
) -> Result<(), TestError> {
    for server_sent in [false, true] {
        let mut mock = MockMailPlus::start_with_sent_policy(&[], server_sent).await?;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let policy = if server_sent {
            "server"
        } else {
            "client_append"
        };
        let account = connect_with_policy(&h, &mock, policy).await?;
        value(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await?,
        )?;
        let editor = h.artifacts.join("accepted-draft.json");
        std::fs::write(&editor,json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"alpha@example.test"}],"subject":"Accepted SMTP recovery","text":"Frozen attachment survives restored positive observation"}).to_string())?;
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
        let pdf = h.artifacts.join("accepted.pdf");
        std::fs::write(&pdf, PDF)?;
        value(
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
        arm(&h, "operation_after_smtp_acceptance")?;
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
            "a8770487-9d5e-4bd9-af59-64bb5f818e66",
        ];
        let queued = value(h.cli(&send).await?)?;
        let operation = queued["id"].as_str().unwrap();
        wait(&h, "operation_after_smtp_acceptance").await?;
        if server_sent {
            mock.control(json!({"command":"wait_server_sent","count":1}))
                .await?;
        }
        let snapshot = effects(&mut mock).await?;
        assert_eq!(snapshot["writes"]["smtp DATA"], json!([1, 1]));
        assert_eq!(snapshot["writes"]["imap APPEND"], json!([0, 0]));
        let backup = h.artifacts.join("accepted.nuncio");
        value(
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
        if !server_sent {
            mock.control(json!({"command":"inject","name":"copy-ack-lost","protocol":"imap","verb":"APPEND","phase":"after","action":"disconnect"})).await?;
        }
        std::fs::write(
            h.artifacts
                .join("barriers/operation_after_smtp_acceptance.release"),
            [],
        )?;
        settled(
            &h,
            &account,
            operation,
            if server_sent { "applied" } else { "uncertain" },
        )
        .await?;
        let restored = value(
            h.cli_with_stdin(
                &[
                    "--json",
                    "backup",
                    "restore",
                    "--file",
                    backup.to_str().unwrap(),
                    "--new-profile",
                    "accepted-recovered",
                ],
                SECRET,
            )
            .await?,
        )?;
        assert_eq!(restored["held_operations"], 1);
        let original = h.directory.clone();
        h.force_kill().await?;
        std::fs::remove_file(editor)?;
        std::fs::remove_file(pdf)?;
        h.directory = restored["directory"].as_str().unwrap().into();
        h.restart().await?;
        assert_eq!(connect_with_policy(&h, &mock, policy).await?, account);
        let held = settled(&h, &account, operation, "uncertain").await?;
        let before = effects(&mut mock).await?;
        assert_eq!(before["writes"]["smtp DATA"], json!([1, 1]));
        assert_eq!(
            before["writes"]["imap APPEND"],
            json!([u64::from(!server_sent), u64::from(!server_sent)])
        );
        assert_eq!(
            before["mailboxes"]["Sent"]["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let version = held["version"].to_string();
        let reconcile = [
            "--json",
            "operation",
            "reconcile",
            "--account",
            &account,
            "--operation",
            operation,
            "--request-id",
            "5ed0a8ee-5c77-48af-b7c2-3f393f1422a8",
            "--version",
            &version,
        ];
        arm(&h, "operation_before_receipt")?;
        value(h.cli(&reconcile).await?)?;
        wait(&h, "operation_before_receipt").await?;
        assert_eq!(effects(&mut mock).await?, before);
        h.force_kill().await?;
        h.restart().await?;
        let applied = settled(&h, &account, operation, "applied").await?;
        assert_eq!(applied["request_id"], queued["request_id"]);
        assert_eq!(value(h.cli(&reconcile).await?)?, applied);
        assert_eq!(effects(&mut mock).await?, before);
        let history = value(
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
        assert_eq!(attempts.len(), 3);
        let original_attempt = attempts.iter().find(|a| a["ordinal"] == 1).unwrap();
        assert_eq!(original_attempt["receipts"].as_array().unwrap().len(), 1);
        assert_eq!(original_attempt["receipts"][0]["kind"], "smtp_accepted");
        assert_eq!(original_attempt["receipts"][0]["source"], "acknowledgement");
        let interrupted = attempts.iter().find(|a| a["ordinal"] == 2).unwrap();
        assert_eq!(interrupted["outcome"], "uncertain");
        assert_eq!(
            interrupted["receipts"].as_array().unwrap().len(),
            usize::from(server_sent)
        );
        let last = attempts.iter().find(|a| a["ordinal"] == 3).unwrap();
        assert_eq!(last["receipts"].as_array().unwrap().len(), 1);
        assert_eq!(last["receipts"][0]["kind"], "sent_copy");
        assert_eq!(last["receipts"][0]["source"], "positive_read");
        let local = list(&h, &account).await?;
        let copies = local["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["subject"] == "Accepted SMTP recovery")
            .collect::<Vec<_>>();
        assert_eq!(copies.len(), 1);
        let output = h.artifacts.join("sent.eml");
        value(
            h.cli(&[
                "--json",
                "mail",
                "raw",
                "--account",
                &account,
                "--message",
                copies[0]["id"].as_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ])
            .await?,
        )?;
        let remote = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":1}),
            )
            .await?;
        let raw = STANDARD.decode(remote["raw_base64"].as_str().unwrap())?;
        assert_eq!(std::fs::read(output)?, raw);
        let sent = mailparse::parse_mail(&raw)?;
        let bcc = mailparse::addrparse(&sent.headers.get_first_value("Bcc").unwrap())?;
        assert_eq!(bcc.len(), 1);
        assert!(matches!(&bcc[0],mailparse::MailAddr::Single(a) if a.addr=="alpha@example.test"));
        assert_eq!(
            sent.subparts
                .iter()
                .find(|p| p.ctype.mimetype == "application/pdf")
                .unwrap()
                .get_body_raw()?,
            PDF
        );
        let delivery = mock.control(json!({"command":"smtp"})).await?;
        assert_eq!(delivery["total"], 1);
        let raw=mock.control(json!({"command":"smtp_raw","transport":"starttls","id":delivery["messages"][0]["ID"]})).await?;
        let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
        let size = before["deliveries"][0]["wire_size"].as_u64().unwrap() as usize;
        assert!(raw.len() >= size);
        let wire = &raw[raw.len() - size..];
        assert_eq!(
            format!("{:x}", Sha256::digest(wire)),
            before["deliveries"][0]["wire_sha256"]
        );
        let wire = mailparse::parse_mail(wire)?;
        assert!(wire.headers.get_first_value("Bcc").is_none());
        assert_eq!(
            wire.subparts
                .iter()
                .find(|p| p.ctype.mimetype == "application/pdf")
                .unwrap()
                .get_body_raw()?,
            PDF
        );
        assert_eq!(
            wire.headers.get_first_value("Message-ID"),
            sent.headers.get_first_value("Message-ID")
        );
        assert_eq!(
            before["deliveries"][0]["recipients"],
            json!(["beta@example.test", "alpha@example.test"])
        );
        assert_eq!(effects(&mut mock).await?, before);
        assert_eq!(std::fs::read(backup)?, ciphertext);
        assert!(original.join("store.db").is_file());
        h.shutdown().await?;
        h.restart().await?;
        assert_eq!(value(h.cli(&reconcile).await?)?, applied);
        assert_eq!(effects(&mut mock).await?, before);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
