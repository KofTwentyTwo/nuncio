use super::imap_read_e2e::{arm, list, wait};
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[tokio::test]
async fn actual_cli_initial_imap_catchup_remains_atomic_across_daemon_crashes(
) -> Result<(), TestError> {
    for (boundary, published) in [
        ("imap-before-promotion", false),
        ("imap-after-promotion", true),
    ] {
        let mut mock = MockMailPlus::start(&[]).await?;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let account = imap_transfer_e2e::connect(&h, &mock).await?;
        arm(&h, "imap-before-message-stage")?;
        arm(&h, boundary)?;
        let run = h
            .cli(&["--json", "sync", "--account", &account, "--full"])
            .await?;
        assert_eq!(run.status, 0);
        wait(&h, "imap-before-message-stage").await?;
        let raw=b"From: sender@example.test\r\nTo: alpha@example.test\r\nSubject: E2E concurrent arrival\r\nMessage-ID: <e2e-concurrent@example.test>\r\n\r\nNew independent bytes\r\n";
        mock.control(json!({"command":"append","account":"alpha@example.test","mailbox":"INBOX","raw_base64":STANDARD.encode(raw)})).await?;
        let remote = imap_effects::effects(&mut mock).await?;
        assert!(list(&h, &account).await?["items"]
            .as_array()
            .unwrap()
            .is_empty());
        std::fs::write(
            h.artifacts
                .join("barriers/imap-before-message-stage.release"),
            [],
        )?;
        wait(&h, boundary).await?;
        let before_crash = list(&h, &account).await?;
        assert_eq!(
            before_crash["items"].as_array().unwrap().len(),
            if published { 3 } else { 0 }
        );
        assert_eq!(imap_effects::effects(&mut mock).await?, remote);
        h.force_kill().await?;
        h.restart().await?;
        let recovered = list(&h, &account).await?;
        assert_eq!(recovered["items"], before_crash["items"]);
        assert_eq!(
            recovered["coverage"]["cursor"],
            before_crash["coverage"]["cursor"]
        );
        let done = h
            .cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?;
        assert_eq!(done.status, 0, "{}", String::from_utf8_lossy(&done.stderr));
        let messages = list(&h, &account).await?;
        assert_eq!(messages["items"].as_array().unwrap().len(), 3);
        let item = messages["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["subject"] == "E2E concurrent arrival")
            .unwrap();
        let file = h.artifacts.join("arrival.eml");
        let downloaded = h
            .cli(&[
                "--json",
                "mail",
                "raw",
                "--account",
                &account,
                "--message",
                item["id"].as_str().unwrap(),
                "--output",
                file.to_str().unwrap(),
            ])
            .await?;
        assert_eq!(downloaded.status, 0);
        assert_eq!(std::fs::read(&file)?, raw);
        assert_eq!(imap_effects::effects(&mut mock).await?, remote);
        h.shutdown().await?;
        let logs = std::fs::read_to_string(h.artifacts.join("daemon-1.stderr.log"))?;
        assert!(logs.contains("IMAP mailbox changed during download"));
        for secret in [
            &mock.credential("alpha@example.test")?,
            "E2E concurrent arrival",
            "sender@example.test",
            "New independent bytes",
        ] {
            assert!(!logs.contains(secret));
        }
        mock.shutdown().await?;
    }
    Ok(())
}
