use super::imap_read_e2e::{arm, list, wait};
use super::imap_transfer_e2e::connect;
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mailparse::MailHeaderMap;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn smtp_and_sent_copy_crashes_preserve_independent_effects_and_frozen_bcc_mime(
) -> Result<(), TestError> {
    for (boundary, expected, accepted_before, copies_before, accepted_after, copies_after) in [
        ("operation_after_attempt", "applied", 0, 0, 1, 1),
        ("operation_after_smtp_start", "uncertain", 0, 0, 0, 0),
        ("operation_after_smtp_acceptance", "applied", 1, 0, 1, 1),
        ("operation_after_sent_append_start", "uncertain", 1, 0, 1, 0),
        ("operation_after_sent_append", "applied", 1, 1, 1, 1),
        ("operation_before_receipt", "applied", 1, 1, 1, 1),
        ("operation_after_sent_rejection", "applied", 1, 0, 1, 1),
    ] {
        let mut mock = MockMailPlus::start(&[]).await?;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let account = connect(&h, &mock).await?;
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await?
                .status,
            0
        );
        let file = h.artifacts.join("smtp-draft.json");
        std::fs::write(&file,json!({"to":[{"address":"beta@example.test"}],"cc":[{"address":"observer@example.test"}],"bcc":[{"address":"blind@example.test"}],"subject":"SMTP crash evidence","text":"Frozen café\n.leading dot\n..two dots\n"}).to_string())?;
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
        let attachment = h.artifacts.join("smtp-attachment.bin");
        let bytes = (0..4096).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        std::fs::write(&attachment, &bytes)?;
        assert_eq!(
            h.cli(&[
                "--json",
                "mail",
                "draft",
                "attach",
                "--account",
                &account,
                "--draft",
                &draft,
                "--version",
                "1",
                "--file",
                attachment.to_str().unwrap(),
                "--mime-type",
                "application/octet-stream"
            ])
            .await?
            .status,
            0
        );
        let rejected_copies = usize::from(boundary == "operation_after_sent_rejection");
        if rejected_copies == 1 {
            mock.control(json!({"command":"inject","name":"rejected-copy","protocol":"imap","verb":"APPEND","phase":"before","action":"reject"})).await?;
        }
        arm(&h, boundary)?;
        let args = [
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "6aa02654-c54f-4a87-b570-9e3610a198cd",
        ];
        let queued = h.cli(&args).await?;
        assert_eq!(queued.status, 0, "{boundary}: {:?}", queued.json());
        let id = queued.json()?["result"]["id"].as_str().unwrap().to_owned();
        wait(&h, boundary).await?;
        effects(&mut mock, accepted_before, copies_before, rejected_copies).await?;
        assert_eq!(
            list(&h, &account).await?["items"].as_array().unwrap().len(),
            2
        );
        h.force_kill().await?;
        h.restart().await?;
        let shown = settled(&h, &account, &id, expected).await?;
        if expected == "uncertain" {
            assert_eq!(
                shown["error_code"],
                if boundary == "operation_after_smtp_start" {
                    "smtp_acceptance_unknown"
                } else {
                    "imap_sent_copy_identity_unknown"
                }
            );
        }
        assert_eq!(h.cli(&args).await?.json()?["result"]["id"], id);
        effects(&mut mock, accepted_after, copies_after, rejected_copies).await?;
        let attempts = h
            .cli(&[
                "--json",
                "operation",
                "attempts",
                "--account",
                &account,
                "--operation",
                &id,
            ])
            .await?;
        assert_eq!(attempts.status, 0);
        let attempts = attempts.json()?;
        let receipts = attempts["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|a| a["receipts"].as_array().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            receipts
                .iter()
                .filter(|r| r["kind"] == "smtp_accepted")
                .count(),
            accepted_after
        );
        assert_eq!(
            receipts.iter().filter(|r| r["kind"] == "sent_copy").count(),
            usize::from(expected == "applied")
        );
        if copies_after == 1 {
            let raw=mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":1})).await?;
            let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
            let parsed = mailparse::parse_mail(&raw)?;
            let bcc = mailparse::addrparse(&parsed.headers.get_first_value("Bcc").unwrap())?;
            assert_eq!(bcc.len(), 1);
            assert!(
                matches!(&bcc[0],mailparse::MailAddr::Single(address) if address.addr=="blind@example.test")
            );
            assert_eq!(
                parsed.headers.get_first_value("Subject").as_deref(),
                Some("SMTP crash evidence")
            );
            let mut pending = vec![&parsed];
            let mut found_attachment = false;
            let mut found_body = false;
            while let Some(part) = pending.pop() {
                if part.ctype.mimetype == "application/octet-stream" {
                    assert_eq!(part.get_body_raw()?, bytes);
                    found_attachment = true;
                }
                if part.ctype.mimetype == "text/plain" {
                    assert_eq!(
                        part.get_body()?.replace("\r\n", "\n"),
                        "Frozen café\n.leading dot\n..two dots\n"
                    );
                    found_body = true;
                }
                pending.extend(&part.subparts);
            }
            assert!(found_attachment && found_body);
            let snapshot = mock.control(json!({"command":"snapshot"})).await?;
            assert_eq!(
                snapshot["smtp_deliveries"][0]["sender"],
                "alpha@example.test"
            );
            assert_eq!(
                snapshot["smtp_deliveries"][0]["recipients"],
                json!([
                    "beta@example.test",
                    "observer@example.test",
                    "blind@example.test"
                ])
            );
            assert_ne!(
                snapshot["smtp_deliveries"][0]["wire_sha256"],
                format!("{:x}", Sha256::digest(&raw)),
                "private Bcc is retained only in the Sent copy"
            );
            let smtp = mock.control(json!({"command":"smtp"})).await?;
            let capture=mock.control(json!({"command":"smtp_raw","transport":"starttls","id":smtp["messages"][0]["ID"]})).await?;
            let capture = STANDARD.decode(capture["raw_base64"].as_str().unwrap())?;
            let wire_size = snapshot["smtp_deliveries"][0]["wire_size"]
                .as_u64()
                .unwrap() as usize;
            let wire = &capture[capture.len() - wire_size..];
            assert_eq!(
                format!("{:x}", Sha256::digest(wire)),
                snapshot["smtp_deliveries"][0]["wire_sha256"]
            );
            let wire = mailparse::parse_mail(wire)?;
            assert!(wire.headers.get_first_value("Bcc").is_none());
            assert_eq!(
                wire.headers.get_first_value("Message-ID"),
                parsed.headers.get_first_value("Message-ID")
            );
            let local = list(&h, &account).await?;
            let message = local["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["subject"] == "SMTP crash evidence")
                .unwrap();
            let downloaded = h.artifacts.join("sent-copy.eml");
            assert_eq!(
                h.cli(&[
                    "--json",
                    "mail",
                    "raw",
                    "--account",
                    &account,
                    "--message",
                    message["id"].as_str().unwrap(),
                    "--output",
                    downloaded.to_str().unwrap()
                ])
                .await?
                .status,
                0
            );
            assert_eq!(std::fs::read(downloaded)?, raw);
        }
        h.force_kill().await?;
        h.restart().await?;
        assert_eq!(h.cli(&args).await?.json()?["result"]["id"], id);
        settled(&h, &account, &id, expected).await?;
        effects(&mut mock, accepted_after, copies_after, rejected_copies).await?;
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
async fn effects(
    mock: &mut MockMailPlus,
    accepted: usize,
    copies: usize,
    rejected_copies: usize,
) -> Result<(), TestError> {
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(
        snapshot["accepted"]["smtp DATA"].as_u64().unwrap_or(0),
        accepted as u64
    );
    assert_eq!(
        snapshot["smtp_deliveries"].as_array().unwrap().len(),
        accepted
    );
    assert_eq!(
        mock.control(json!({"command":"smtp"})).await?["total"],
        accepted
    );
    assert_eq!(
        snapshot["requests"]["imap APPEND"].as_u64().unwrap_or(0),
        (copies + rejected_copies) as u64
    );
    assert_eq!(
        snapshot["accepted"]["imap APPEND"].as_u64().unwrap_or(0),
        copies as u64
    );
    let sent = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
        .await?;
    assert_eq!(sent["messages"].as_array().unwrap().len(), copies);
    Ok(())
}
pub(super) async fn settled(
    h: &E2eHarness,
    account: &str,
    id: &str,
    expected: &str,
) -> Result<serde_json::Value, TestError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
    loop {
        let shown = h
            .cli(&[
                "--json",
                "operation",
                "show",
                "--account",
                account,
                "--operation",
                id,
            ])
            .await?;
        assert_eq!(shown.status, 0);
        let shown = shown.json()?["result"].clone();
        if shown["state"] == expected && shown["needs_reconciliation"] == false {
            return Ok(shown);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {expected}, got {shown}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
