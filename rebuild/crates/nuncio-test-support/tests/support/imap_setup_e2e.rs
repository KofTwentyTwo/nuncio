use nuncio_test_support::{google::Seed, imap::MockMailPlus, process::E2eHarness, TestError};
use serde_json::json;
#[path = "terminal.rs"]
mod terminal;

#[tokio::test]
async fn guided_mailplus_setup_checks_password_and_preserves_remote_state() -> Result<(), TestError>
{
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let before = super::imap_effects::effects(&mut mock).await?;
    let password = mock.credential("alpha@example.test")?;
    for accepted in [false, true] {
        let result = terminal::run(&h, &["account", "add"], json!([
            {"prompt":"Provider", "answer":"2"}, {"prompt":"Email address", "answer":"alpha@example.test"},
            {"prompt":"Mail server", "answer":"127.0.0.1"}, {"prompt":"Username", "answer":""},
            {"prompt":"Password", "secret":true, "answer":if accepted {password.as_str()} else {"synthetic-incorrect-password"}},
            {"prompt":"Change advanced server settings?", "answer":"yes"},
            {"prompt":"IMAP port", "answer":mock.ready.ports.imaps.to_string()},
            {"prompt":"IMAP security", "answer":"1"}, {"prompt":"SMTP server", "answer":""},
            {"prompt":"SMTP port", "answer":mock.ready.ports.smtp.to_string()},
            {"prompt":"SMTP security", "answer":"2"}, {"prompt":"SMTP username", "answer":""},
            {"prompt":"Different SMTP password?", "answer":"no"},
            {"prompt":"Custom CA certificate file", "answer":mock.ready.ca_file},
            {"prompt":"Sent folder", "answer":""}, {"prompt":"Does the SMTP server save its own Sent copy?", "answer":"no"},
            {"prompt":"Archive folder", "answer":"Archive"}, {"prompt":"Trash folder", "answer":"Trash"},
            {"prompt":"Connect and start syncing?", "answer":"yes"}
        ]), None).await?;
        assert_eq!(
            result["exit_status"],
            if accepted { 0 } else { 4 },
            "{result}"
        );
        assert_eq!(result["secrets_hidden"], 1);
        assert_eq!(result["echo_restored"], true);
        let accounts =
            h.cli(&["--json", "account", "list"]).await?.json()?["result"]["accounts"].clone();
        assert_eq!(accounts.as_array().unwrap().len(), usize::from(accepted));
        if accepted {
            let id = accounts[0]["id"].as_str().unwrap();
            let config = h
                .cli(&["--json", "account", "imap-config", "--account", id])
                .await?
                .json()?["result"]["config"]
                .clone();
            assert_eq!(config["imap"]["port"], mock.ready.ports.imaps);
            assert_eq!(config["smtp"]["port"], mock.ready.ports.smtp);
            assert_eq!(config["imap"]["username"], "alpha@example.test");
            assert_eq!(config["archive_folder"], "Archive");
            assert_eq!(
                config["trusted_ca_pem"],
                tokio::fs::read_to_string(&mock.ready.ca_file).await?
            );
            h.force_kill().await?;
            h.restart().await?;
            assert_eq!(
                h.cli(&["account", "check", "--account", id]).await?.status,
                0
            );
        }
        assert_eq!(super::imap_effects::effects(&mut mock).await?, before);
    }
    h.shutdown().await?;
    h.google.stop().await?;
    mock.shutdown().await?;
    Ok(())
}
