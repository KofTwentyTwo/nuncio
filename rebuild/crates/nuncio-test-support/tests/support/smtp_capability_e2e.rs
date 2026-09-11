use super::*;
use super::{
    imap_read_e2e::list, imap_transfer_e2e::connect_with_policy, smtp_e2e::settled,
    smtp_resolution_evidence::observe,
};

#[tokio::test]
async fn missing_uidplus_cli_refuses_client_copy_but_can_observe_server_sent(
) -> Result<(), TestError> {
    for server_sent in [false, true] {
        let mut mock = MockMailPlus::start_with_sent_policy(&["UIDPLUS"], server_sent).await?;
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
        let configuration = h
            .cli(&["--json", "account", "imap-config", "--account", &account])
            .await?;
        assert_eq!(configuration.status, 0);
        assert_eq!(
            configuration.json()?["result"]["capabilities"]["uidplus"],
            false
        );
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await?
                .status,
            0
        );
        let file = h.artifacts.join("uidplus-draft.json");
        std::fs::write(&file,json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"blind@example.test"}],"subject":"SMTP resolution evidence","text":"Independent acceptance and copy\n.leading dot\n"}).to_string())?;
        let saved = h
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
        assert_eq!(saved.status, 0);
        let draft = saved.json()?["result"]["id"].as_str().unwrap().to_owned();
        let args = [
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "785895b5-e3d5-4af2-ac80-281b3139532b",
        ];
        let send = h.cli(&args).await?;
        assert_eq!(send.status, 0);
        let id = send.json()?["result"]["id"].as_str().unwrap().to_owned();
        let expected = if server_sent { "applied" } else { "failed" };
        let operation = settled(&h, &account, &id, expected).await?;
        if !server_sent {
            assert_eq!(operation["error_code"], "imap_sent_uidplus_required");
            assert!(operation["next_attempt_at_ms"].is_null());
        }
        h.force_kill().await?;
        h.restart().await?;
        assert_eq!(h.cli(&args).await?.json()?["result"], operation);
        assert_eq!(settled(&h, &account, &id, expected).await?, operation);
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
        let attempts = attempts.json()?["result"]["items"].clone();
        let receipts = attempts
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|a| a["receipts"].as_array().unwrap())
            .collect::<Vec<_>>();
        for kind in ["smtp_accepted", "sent_copy", "server_sent_observed"] {
            assert_eq!(
                receipts.iter().filter(|r| r["kind"] == kind).count(),
                usize::from(server_sent)
            );
        }
        if server_sent {
            let observed = observe(&mut mock, true).await?;
            let local = list(&h, &account).await?;
            let local = local["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["subject"] == "SMTP resolution evidence")
                .unwrap();
            observed.check_placement(local["provider_id"].as_str().unwrap(), &account)?;
            let output = h.artifacts.join("server-sent-without-uidplus.eml");
            assert_eq!(
                h.cli(&[
                    "--json",
                    "mail",
                    "raw",
                    "--account",
                    &account,
                    "--message",
                    local["id"].as_str().unwrap(),
                    "--output",
                    output.to_str().unwrap()
                ])
                .await?
                .status,
                0
            );
            assert_eq!(std::fs::read(output)?, observed.raw);
        } else {
            assert_eq!(attempts.as_array().unwrap().len(), 1);
            assert_eq!(attempts[0]["outcome"], "rejected");
            let snapshot = mock.control(json!({"command":"snapshot"})).await?;
            for key in ["requests", "accepted"] {
                for command in ["smtp DATA", "imap APPEND"] {
                    assert_eq!(snapshot[key][command].as_u64().unwrap_or(0), 0);
                }
            }
            assert_eq!(snapshot["smtp_deliveries"], json!([]));
            assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
            assert_eq!(
                mock.control(
                    json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"})
                )
                .await?["messages"],
                json!([])
            );
        }
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
