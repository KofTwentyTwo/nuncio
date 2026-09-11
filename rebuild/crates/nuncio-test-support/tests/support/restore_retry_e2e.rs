use super::{result, E2eHarness, Seed, Value, SECRET};
use nuncio_test_support::TestError;

#[tokio::test]
async fn actual_cli_failed_restores_allow_corrected_passphrase_without_daemon_restart(
) -> Result<(), TestError> {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    h.connect_google("alpha@example.test").await?;
    let original_status = result(h.cli(&["--json", "system", "status"]).await?);
    let original_profile = std::fs::read(h.directory.join("profile.json"))?;
    let backup = h.artifacts.join("retry.nuncio");
    result(
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
    );
    let original_backup = std::fs::read(&backup)?;
    let original_keys: Value = serde_json::from_slice(&std::fs::read(&h.secrets_file)?)?;
    let remote = serde_json::to_value(h.google.control().snapshot().await)?;
    let target = h.directory.parent().unwrap().join("retry-recovered");
    let args = [
        "--json",
        "backup",
        "restore",
        "--file",
        backup.to_str().unwrap(),
        "--new-profile",
        "retry-recovered",
    ];
    for attempt in 1..=65 {
        let failed = h
            .cli_with_stdin(
                &args,
                br#"{"passphrase":"incorrect synthetic recovery phrase"}"#,
            )
            .await?;
        assert_eq!(
            failed.status,
            2,
            "wrong-passphrase attempt {attempt}: {}",
            String::from_utf8_lossy(&failed.stderr)
        );
        assert!(!target.exists());
        let keys: Value = serde_json::from_slice(&std::fs::read(&h.secrets_file)?)?;
        assert_eq!(keys, original_keys);
    }
    let restored = result(h.cli_with_stdin(&args, SECRET).await?);
    assert_ne!(restored["profile_id"], original_status["profile_id"]);
    assert!(target.is_dir());
    let keys: Value = serde_json::from_slice(&std::fs::read(&h.secrets_file)?)?;
    for (name, value) in original_keys.as_object().unwrap() {
        assert!(keys.get(name) == Some(value), "original key entry changed");
    }
    assert_eq!(
        keys.as_object().unwrap().len(),
        original_keys.as_object().unwrap().len() + 2
    );
    assert_eq!(std::fs::read(&backup)?, original_backup);
    assert_eq!(
        std::fs::read(h.directory.join("profile.json"))?,
        original_profile
    );
    assert_eq!(
        result(h.cli(&["--json", "system", "status"]).await?)["profile_id"],
        original_status["profile_id"]
    );
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await)?,
        remote
    );
    h.shutdown().await?;
    h.directory = target;
    h.restart().await?;
    assert_eq!(
        result(h.cli(&["--json", "system", "status"]).await?)["profile_id"],
        restored["profile_id"]
    );
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await)?,
        remote
    );
    h.shutdown().await?;
    Ok(())
}
