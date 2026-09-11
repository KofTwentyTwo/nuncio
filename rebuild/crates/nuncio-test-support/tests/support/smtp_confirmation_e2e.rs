use super::*;
use super::{
    imap_read_e2e::list, imap_transfer_e2e::connect_with_policy, smtp_e2e::settled,
    smtp_resolution_evidence::observe,
};

#[tokio::test]
async fn smtp_confirmation_and_abandonment_cli_keep_unknown_transport_history_across_restart(
) -> Result<(), TestError> {
    for (server_sent, confirm) in [(false, false), (false, true), (true, false), (true, true)] {
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
        let file = h.artifacts.join("resolution-draft.json");
        std::fs::write(&file,json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"blind@example.test"}],"subject":"SMTP resolution evidence","text":"Independent acceptance and copy\n.leading dot\n"}).to_string())?;
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
        mock.control(json!({"command":"inject","name":"lost-ack","protocol":if server_sent {"smtp"}else{"imap"},"verb":if server_sent {"DATA"}else{"APPEND"},"phase":"after","action":"disconnect"})).await?;
        let send = [
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "6d64e04b-e6f7-40e3-8b22-d4ecce079271",
        ];
        let queued = h.cli(&send).await?;
        assert_eq!(queued.status, 0);
        let id = queued.json()?["result"]["id"].as_str().unwrap().to_owned();
        let unknown = settled(&h, &account, &id, "uncertain").await?;
        assert_eq!(
            unknown["error_code"],
            if server_sent {
                "smtp_acceptance_unknown"
            } else {
                "imap_sent_copy_identity_unknown"
            }
        );
        if server_sent {
            mock.control(json!({"command":"wait_server_sent","count":1}))
                .await?;
        }
        let observed = observe(&mut mock, server_sent).await?;
        let desired: serde_json::Value =
            serde_json::from_str(unknown["desired_state_json"].as_str().unwrap())?;
        assert_eq!(desired["message_id"], observed.message_id);
        h.force_kill().await?;
        h.restart().await?;
        assert_eq!(settled(&h, &account, &id, "uncertain").await?, unknown);
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await?
                .status,
            0
        );
        let local = list(&h, &account).await?;
        let local = local["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["subject"] == "SMTP resolution evidence")
            .unwrap();
        let provider_id = local["provider_id"].as_str().unwrap();
        observed.check_placement(provider_id, &account)?;
        let output = h.artifacts.join("independently-observed-sent.eml");
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
        assert_eq!(settled(&h, &account, &id, "uncertain").await?, unknown);
        let attempt_args = [
            "--json",
            "operation",
            "attempts",
            "--account",
            &account,
            "--operation",
            &id,
        ];
        let before = h.cli(&attempt_args).await?;
        assert_eq!(before.status, 0);
        let before = before.json()?["result"]["items"].clone();
        let receipts = before
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
            usize::from(!server_sent)
        );
        assert!(receipts
            .iter()
            .all(|r| r["kind"] != "sent_copy" && r["kind"] != "server_sent_observed"));
        let version = unknown["version"].as_u64().unwrap().to_string();
        let file = h.artifacts.join("smtp-resolution.json");
        let resolve = [
            "--json",
            "operation",
            "resolve",
            "--account",
            &account,
            "--operation",
            &id,
            "--version",
            &version,
            "--file",
            file.to_str().unwrap(),
        ];
        let mut decision = json!({"decision":"confirm_applied","evidence":{"provider_id":provider_id,"message_id":observed.message_id,"observed_at_ms":unknown["updated_at_ms"],"note":"Verified Sent placement and exact downloaded MIME against independent Dovecot"}});
        std::fs::write(&file, decision.to_string())?;
        let refused = h.cli(&resolve).await?;
        assert_eq!(refused.status, 2);
        assert_eq!(settled(&h, &account, &id, "uncertain").await?, unknown);
        decision["evidence"]["smtp_acceptance_note"] =
            json!("Verified independent Mailpit acceptance and captured SMTP envelope");
        if !confirm {
            decision = json!({"decision":"abandon","reason":"Preserve uncertainty and stop reconciliation without another send"});
        }
        std::fs::write(&file, decision.to_string())?;
        let resolved = h.cli(&resolve).await?;
        assert_eq!(resolved.status, 0, "{:?}", resolved.json());
        let resolved = resolved.json()?["result"].clone();
        assert_eq!(
            resolved["state"],
            if confirm { "applied" } else { "uncertain" }
        );
        assert_eq!(
            resolved["disposition"],
            if confirm {
                "manual_confirmed"
            } else {
                "abandoned"
            }
        );
        assert_eq!(
            resolved["error_code"],
            if confirm {
                serde_json::Value::Null
            } else {
                unknown["error_code"].clone()
            }
        );
        assert_eq!(resolved["resolutions"].as_array().unwrap().len(), 1);
        assert_eq!(resolved["needs_reconciliation"], false);
        h.force_kill().await?;
        h.restart().await?;
        let again = h.cli(&resolve).await?;
        assert_eq!(again.status, 0);
        assert_eq!(again.json()?["result"], resolved);
        assert_eq!(h.cli(&send).await?.json()?["result"]["id"], id);
        let after = h.cli(&attempt_args).await?;
        assert_eq!(after.status, 0);
        assert_eq!(after.json()?["result"]["items"], before);
        std::fs::write(
            &file,
            json!({"decision":"abandon","reason":"A conflicting later decision"}).to_string(),
        )?;
        let conflicting = h.cli(&resolve).await?;
        assert_eq!(conflicting.status, 5);
        assert_eq!(conflicting.json()?["error"]["code"], "conflict");
        assert_eq!(observe(&mut mock, server_sent).await?.raw, observed.raw);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
