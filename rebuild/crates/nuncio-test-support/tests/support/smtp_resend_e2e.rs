use super::*;
use super::{imap_transfer_e2e::connect_with_policy, smtp_e2e::settled};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mailparse::MailHeaderMap;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn smtp_resend_requires_duplicate_risk_and_uses_new_durable_intent_after_draft_deletion(
) -> Result<(), TestError> {
    for server_sent in [false, true] {
        let mut mock = MockMailPlus::start_with_sent_policy(&[], server_sent).await?;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let account = connect_with_policy(
            &h,
            &mock,
            if server_sent {
                "server"
            } else {
                "client_append"
            },
        )
        .await?;
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await?
                .status,
            0
        );
        let file = h.artifacts.join("resend-draft.json");
        std::fs::write(&file,json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"blind@example.test"}],"subject":"Explicit SMTP resend","text":"Original frozen body\n.leading dot"}).to_string())?;
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
        let attachment = h.artifacts.join("resend.bin");
        let bytes = (0..1024).map(|n| (n % 251) as u8).collect::<Vec<_>>();
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
        mock.control(json!({"command":"inject","name":"lost-first-smtp","protocol":"smtp","verb":"DATA","phase":"after","action":"disconnect"})).await?;
        let send = [
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "5b2d473a-84d0-49be-9aa2-e05531b47611",
        ];
        let queued = h.cli(&send).await?;
        assert_eq!(queued.status, 0);
        let original = queued.json()?["result"]["id"].as_str().unwrap().to_owned();
        let unknown = settled(&h, &account, &original, "uncertain").await?;
        assert_eq!(unknown["error_code"], "smtp_acceptance_unknown");
        if server_sent {
            mock.control(json!({"command":"wait_server_sent","count":1}))
                .await?;
        }
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 1);
        assert_eq!(
            h.cli(&[
                "--json",
                "mail",
                "draft",
                "delete",
                "--account",
                &account,
                "--draft",
                &draft,
                "--version",
                "2"
            ])
            .await?
            .status,
            0
        );
        std::fs::remove_file(&attachment)?;
        h.force_kill().await?;
        h.restart().await?;
        let unknown = settled(&h, &account, &original, "uncertain").await?;
        let version = unknown["version"].as_u64().unwrap().to_string();
        let file = h.artifacts.join("resend-decision.json");
        std::fs::write(&file,json!({"decision":"resend","request_id":"4a7f64ea-13f5-4257-9d2d-b58f5573cf09","reason":"Accept possible duplicate delivery of frozen content"}).to_string())?;
        let mut resolve = vec![
            "--json",
            "operation",
            "resolve",
            "--account",
            &account,
            "--operation",
            &original,
            "--version",
            &version,
            "--file",
            file.to_str().unwrap(),
        ];
        let rejected = h.cli(&resolve).await?;
        assert_eq!(rejected.status, 2);
        assert_eq!(
            rejected.json()?["error"]["code"],
            "duplicate_risk_not_accepted"
        );
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 1);
        resolve.push("--accept-duplicate-risk");
        let resolved = h.cli(&resolve).await?;
        assert_eq!(resolved.status, 0);
        let resolved = resolved.json()?["result"].clone();
        assert_eq!(resolved["state"], "uncertain");
        assert_eq!(resolved["disposition"], "resend_requested");
        let replacement = resolved["resolutions"][0]["replacement_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(replacement, original);
        settled(&h, &account, &replacement, "applied").await?;
        h.force_kill().await?;
        h.restart().await?;
        let retry = h.cli(&resolve).await?;
        assert_eq!(retry.status, 0);
        assert_eq!(retry.json()?["result"], resolved);
        assert_eq!(h.cli(&send).await?.json()?["result"]["id"], original);
        settled(&h, &account, &replacement, "applied").await?;
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(snapshot["requests"]["smtp DATA"], 2);
        assert_eq!(snapshot["accepted"]["smtp DATA"], 2);
        let envelopes = snapshot["smtp_deliveries"].as_array().unwrap();
        assert_eq!(envelopes.len(), 2);
        let smtp = mock.control(json!({"command":"smtp"})).await?;
        assert_eq!(smtp["total"], 2);
        let mut ids = std::collections::BTreeSet::new();
        let mut bodies = Vec::new();
        for capture in smtp["messages"].as_array().unwrap() {
            assert_eq!(capture["Username"], "alpha@example.test");
            let raw = mock
                .control(json!({"command":"smtp_raw","transport":"starttls","id":capture["ID"]}))
                .await?;
            let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
            let envelope = envelopes
                .iter()
                .find(|e| {
                    let size = e["wire_size"].as_u64().unwrap() as usize;
                    raw.len() >= size
                        && format!("{:x}", Sha256::digest(&raw[raw.len() - size..]))
                            == e["wire_sha256"]
                })
                .unwrap();
            assert_eq!(envelope["sender"], "alpha@example.test");
            assert_eq!(
                envelope["recipients"],
                json!(["beta@example.test", "blind@example.test"])
            );
            let wire = &raw[raw.len() - envelope["wire_size"].as_u64().unwrap() as usize..];
            let parsed = mailparse::parse_mail(wire)?;
            assert!(parsed.headers.get_first_value("Bcc").is_none());
            assert!(ids.insert(parsed.headers.get_first_value("Message-ID").unwrap()));
            assert_eq!(parsed.subparts[1].get_body_raw()?, bytes);
            let body = wire.windows(4).position(|b| b == b"\r\n\r\n").unwrap() + 4;
            bodies.push(wire[body..].to_vec());
        }
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
        assert_eq!(
            snapshot["requests"]["imap APPEND"].as_u64().unwrap_or(0),
            u64::from(!server_sent)
        );
        assert_eq!(
            snapshot["accepted"]["imap APPEND"].as_u64().unwrap_or(0),
            u64::from(!server_sent)
        );
        let sent = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
            .await?;
        assert_eq!(
            sent["messages"].as_array().unwrap().len(),
            if server_sent { 2 } else { 1 }
        );
        for id in [&original, &replacement] {
            let attempts = h
                .cli(&[
                    "--json",
                    "operation",
                    "attempts",
                    "--account",
                    &account,
                    "--operation",
                    id,
                ])
                .await?;
            assert_eq!(attempts.status, 0);
            let attempts = attempts.json()?["result"].clone();
            let receipts = attempts["items"]
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
                usize::from(id == &replacement)
            );
            assert_eq!(
                receipts.iter().filter(|r| r["kind"] == "sent_copy").count(),
                usize::from(id == &replacement)
            );
        }
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
