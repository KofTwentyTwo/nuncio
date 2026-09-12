use super::{imap_effects, E2eHarness, MockMailPlus, Seed, TestError};
use serde_json::json;

#[tokio::test]
async fn imap_account_edit_archive_restore_reauth_and_purge_preserve_remote_mail(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let mut config = json!({"schema_version":1,"address":"alpha@example.test","imap":{"host":"127.0.0.1","port":mock.ready.ports.imaps,"tls":"implicit","username":"alpha@example.test"},"smtp":{"host":"127.0.0.1","port":mock.ready.ports.smtp,"tls":"start_tls","username":"alpha@example.test"},"sent_policy":"client_append","sent_folder":"Sent","archive_folder":"Archive","trash_folder":"Trash","trusted_ca_pem":tokio::fs::read_to_string(&mock.ready.ca_file).await?});
    let file = h.artifacts.join("account-public.json");
    std::fs::write(&file, config.to_string())?;
    let password = mock.credential("alpha@example.test")?;
    let secret = zeroize::Zeroizing::new(
        json!({"imap_password":password,"smtp_password":password}).to_string(),
    );
    let connected = h
        .cli_with_stdin(
            &[
                "--json",
                "account",
                "add-imap",
                "--config",
                file.to_str().unwrap(),
                "--credentials-stdin",
            ],
            secret.as_bytes(),
        )
        .await?;
    assert_eq!(connected.status, 0);
    let connected = connected.json()?["result"]["account"].clone();
    let id = connected["id"].as_str().unwrap();
    let version = connected["version"].as_u64().unwrap().to_string();
    let effects = imap_effects::effects(&mut mock).await?;
    config["address"] = json!("alias@example.test");
    std::fs::write(&file, config.to_string())?;
    let edited = h
        .cli(&[
            "--json",
            "account",
            "edit-imap",
            "--account",
            id,
            "--version",
            &version,
            "--config",
            file.to_str().unwrap(),
        ])
        .await?;
    assert_eq!(
        edited.status, 0,
        "saved public IMAP settings must be editable"
    );
    let edited = edited.json()?["result"]["account"].clone();
    assert_eq!(edited["address"], "alias@example.test");
    assert_eq!(edited["identity"], connected["identity"]);
    assert_eq!(
        h.cli(&[
            "--json",
            "account",
            "edit-imap",
            "--account",
            id,
            "--version",
            &version,
            "--config",
            file.to_str().unwrap()
        ])
        .await?
        .status,
        5
    );
    config["imap"]["username"] = json!("beta@example.test");
    std::fs::write(&file, config.to_string())?;
    let current = edited["version"].as_u64().unwrap().to_string();
    assert_ne!(
        h.cli(&[
            "--json",
            "account",
            "edit-imap",
            "--account",
            id,
            "--version",
            &current,
            "--config",
            file.to_str().unwrap()
        ])
        .await?
        .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "sync", "--account", id, "--wait"])
            .await?
            .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "account", "pause", "--account", id])
            .await?
            .status,
        0
    );
    h.force_kill().await?;
    h.restart().await?;
    assert_ne!(
        h.cli(&["--json", "account", "check", "--account", id])
            .await?
            .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "account", "resume", "--account", id])
            .await?
            .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "account", "remove", "--account", id])
            .await?
            .status,
        0
    );
    let archived = h
        .cli(&["--json", "mail", "list", "--account", id])
        .await?
        .json()?;
    let messages = archived["result"]["items"].as_array().unwrap().len();
    assert!(messages > 0);
    assert_ne!(
        h.cli_with_stdin(
            &[
                "--json",
                "account",
                "reauth-imap",
                "--account",
                id,
                "--credentials-stdin"
            ],
            secret.as_bytes()
        )
        .await?
        .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "account", "restore", "--account", id])
            .await?
            .status,
        0
    );
    let reauth = h
        .cli_with_stdin(
            &[
                "--json",
                "account",
                "reauth-imap",
                "--account",
                id,
                "--credentials-stdin",
            ],
            secret.as_bytes(),
        )
        .await?;
    assert_eq!(reauth.status, 0);
    assert_eq!(
        reauth.json()?["result"]["account"]["address"],
        "alias@example.test"
    );
    assert_eq!(
        h.cli(&["--json", "account", "remove", "--account", id])
            .await?
            .status,
        0
    );
    let preview = h
        .cli(&["--json", "account", "purge", "--account", id, "--dry-run"])
        .await?;
    assert_eq!(preview.status, 0);
    assert_eq!(preview.json()?["result"]["messages"], messages);
    assert_eq!(
        h.cli(&[
            "--json",
            "account",
            "purge",
            "--account",
            id,
            "--confirm",
            id
        ])
        .await?
        .status,
        0
    );
    assert_eq!(imap_effects::effects(&mut mock).await?, effects);
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
