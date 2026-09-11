use super::*;
use super::{
    imap_read_e2e::{arm, list, wait},
    imap_transfer_e2e::connect_with_policy,
    smtp_e2e::settled,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mailparse::MailHeaderMap;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn server_sent_crashes_and_ambiguous_copies_never_repeat_delivery_or_append(
) -> Result<(), TestError> {
    for scenario in [
        "operation_after_smtp_acceptance",
        "operation_after_server_sent",
        "operation_before_receipt",
        "duplicate",
        "lost_data_ack",
    ] {
        let mut mock = MockMailPlus::start_with_sent_policy(&[], true).await?;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let account = connect_with_policy(&h, &mock, "server").await?;
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await?
                .status,
            0
        );
        let file = h.artifacts.join("server-sent-draft.json");
        std::fs::write(&file,json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"blind@example.test"}],"subject":"Server Sent recovery","text":"Frozen server content\n.leading dot"}).to_string())?;
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
        let bytes = (0..1024).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        let attachment = h.artifacts.join("server-evidence.bin");
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
        let boundary = if scenario == "duplicate" {
            "operation_after_smtp_acceptance"
        } else {
            scenario
        };
        if scenario == "lost_data_ack" {
            mock.control(json!({"command":"inject","name":"smtp-ack","protocol":"smtp","verb":"DATA","phase":"after","action":"withhold"})).await?;
        } else {
            arm(&h, boundary)?;
        }
        let args = [
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "4bc53b29-b697-43ac-ad1d-3f291594c33c",
        ];
        let queued = h.cli(&args).await?;
        assert_eq!(queued.status, 0, "{scenario}: {:?}", queued.json());
        let id = queued.json()?["result"]["id"].as_str().unwrap().to_owned();
        if scenario == "lost_data_ack" {
            mock.control(json!({"command":"wait_fault","name":"smtp-ack"}))
                .await?;
        } else {
            wait(&h, boundary).await?;
        }
        assert_eq!(
            mock.control(json!({"command":"wait_server_sent","count":1}))
                .await?["completed"],
            1
        );
        let sent = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
            .await?;
        assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
        let raw=mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":sent["messages"][0]["uid"]})).await?;
        let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
        let smtp = mock.control(json!({"command":"smtp"})).await?;
        assert_eq!(smtp["total"], 1);
        let capture = mock
            .control(
                json!({"command":"smtp_raw","transport":"starttls","id":smtp["messages"][0]["ID"]}),
            )
            .await?;
        assert_eq!(
            raw,
            STANDARD.decode(capture["raw_base64"].as_str().unwrap())?
        );
        if scenario == "duplicate" {
            mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"Sent","uid":sent["messages"][0]["uid"],"destination":"Sent"})).await?;
        }
        assert_eq!(
            list(&h, &account).await?["items"].as_array().unwrap().len(),
            2
        );
        h.force_kill().await?;
        if scenario == "lost_data_ack" {
            mock.control(json!({"command":"release","name":"smtp-ack"}))
                .await?;
        }
        h.restart().await?;
        let expected = if matches!(scenario, "duplicate" | "lost_data_ack") {
            "uncertain"
        } else {
            "applied"
        };
        let operation = settled(&h, &account, &id, expected).await?;
        if expected == "uncertain" {
            assert_eq!(
                operation["error_code"],
                if scenario == "duplicate" {
                    "smtp_server_sent_ambiguous"
                } else {
                    "smtp_acceptance_unknown"
                }
            );
        }
        assert_eq!(h.cli(&args).await?.json()?["result"]["id"], id);
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
            usize::from(scenario != "lost_data_ack")
        );
        assert_eq!(
            receipts
                .iter()
                .filter(|r| r["kind"] == "server_sent_observed")
                .count(),
            usize::from(expected == "applied")
        );
        assert_eq!(
            receipts.iter().filter(|r| r["kind"] == "sent_copy").count(),
            usize::from(expected == "applied")
        );
        assert!(receipts.iter().all(|r| r["kind"] != "imap_append"));
        if expected == "applied" {
            let local = list(&h, &account).await?;
            let message = local["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["subject"] == "Server Sent recovery")
                .unwrap();
            let download = h.artifacts.join("server-copy.eml");
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
                    download.to_str().unwrap()
                ])
                .await?
                .status,
                0
            );
            assert_eq!(std::fs::read(download)?, raw);
        }
        let parsed = mailparse::parse_mail(&raw)?;
        let mut pending = vec![&parsed];
        let mut binary = false;
        while let Some(part) = pending.pop() {
            if part.ctype.mimetype == "application/octet-stream" {
                assert_eq!(part.get_body_raw()?, bytes);
                binary = true;
            }
            pending.extend(&part.subparts);
        }
        assert!(binary);
        assert_eq!(
            parsed.headers.get_first_value("Subject").as_deref(),
            Some("Server Sent recovery")
        );
        h.force_kill().await?;
        h.restart().await?;
        settled(&h, &account, &id, expected).await?;
        assert_eq!(h.cli(&args).await?.json()?["result"]["id"], id);
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        assert_eq!(snapshot["requests"]["smtp DATA"], 1);
        assert_eq!(snapshot["accepted"]["smtp DATA"], 1);
        assert_eq!(snapshot["smtp_deliveries"].as_array().unwrap().len(), 1);
        let delivery = &snapshot["smtp_deliveries"][0];
        assert_eq!(delivery["sender"], "alpha@example.test");
        assert_eq!(
            delivery["recipients"],
            json!(["beta@example.test", "blind@example.test"])
        );
        let size = delivery["wire_size"].as_u64().unwrap() as usize;
        let wire = &raw[raw.len() - size..];
        assert_eq!(
            format!("{:x}", Sha256::digest(wire)),
            delivery["wire_sha256"]
        );
        assert!(mailparse::parse_mail(wire)?
            .headers
            .get_first_value("Bcc")
            .is_none());
        assert_eq!(snapshot["requests"]["imap APPEND"].as_u64().unwrap_or(0), 0);
        assert_eq!(snapshot["accepted"]["imap APPEND"].as_u64().unwrap_or(0), 0);
        let remote = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
            .await?;
        assert_eq!(
            remote["messages"].as_array().unwrap().len(),
            if scenario == "duplicate" { 2 } else { 1 }
        );
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 1);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
