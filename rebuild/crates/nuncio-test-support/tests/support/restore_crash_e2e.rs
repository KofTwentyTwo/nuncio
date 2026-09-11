use super::{result, E2eHarness, Seed, SECRET};
use nuncio_test_support::{
    process::{binary, isolated_command},
    TestError,
};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;
fn stages(parent: &Path, prefix: &str) -> Result<Vec<std::path::PathBuf>, TestError> {
    let mut result = vec![];
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(prefix) {
            result.push(entry.path());
        }
    }
    result.sort();
    Ok(result)
}
#[tokio::test]
async fn restore_sigkill_cleans_owned_staging_and_new_keys_but_preserves_activated_profiles(
) -> Result<(), TestError> {
    for (boundary, activated, new_keys) in [
        ("restore_stage_owned", false, 0),
        ("restore_after_stage", false, 0),
        ("restore_after_database_key", false, 1),
        ("restore_after_api_key", false, 2),
        ("restore_after_activation", true, 2),
    ] {
        let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
        let account = h.connect_google("alpha@example.test").await?;
        let editor = h.artifacts.join("restore-crash-draft.json");
        std::fs::write(&editor,json!({"to":[{"address":"recipient@example.test"}],"subject":"Restore crash preservation","text":"Durable original draft"}).to_string())?;
        let saved = result(
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
        );
        let draft = saved["id"].as_str().unwrap();
        let pdf = h.artifacts.join("restore-crash.pdf");
        std::fs::write(
            &pdf,
            include_bytes!("../../../../tests/fixtures/recovery/queued-draft.pdf"),
        )?;
        let original_draft = result(
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
        );
        let backup = h.artifacts.join("restore-crash.nuncio");
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
        let original_profile = std::fs::read(h.directory.join("profile.json"))?;
        let original_keys: Value = serde_json::from_slice(&std::fs::read(&h.secrets_file)?)?;
        let remote = serde_json::to_value(h.google.control().snapshot().await)?;
        let target = h.directory.parent().unwrap().join("crash-recovered");
        let parent = h.directory.parent().unwrap().to_owned();
        assert!(stages(&parent, ".restore-")?.is_empty());
        assert!(stages(&h.directory, ".maintenance-")?.is_empty());
        let barriers = h.artifacts.join("barriers");
        std::fs::write(barriers.join(format!("{boundary}.arm")), [])?;
        let stdout = h.artifacts.join("restore-crash.stdout.log");
        let mut cli = isolated_command(&binary("NUNCIO_E2E_CLI")?, &h.artifacts.join("tmp"))
            .arg("--endpoint")
            .arg(&h.endpoint)
            .arg("--data-dir")
            .arg(&h.directory)
            .arg("--test-secrets-file")
            .arg(&h.secrets_file)
            .args([
                "--json",
                "backup",
                "restore",
                "--file",
                backup.to_str().unwrap(),
                "--new-profile",
                "crash-recovered",
            ])
            .stdin(Stdio::piped())
            .stdout(std::fs::File::create(&stdout)?)
            .stderr(std::fs::File::create(
                h.artifacts.join("restore-crash.stderr.log"),
            )?)
            .spawn()?;
        let mut input = cli.stdin.take().unwrap();
        input.write_all(SECRET).await?;
        drop(input);
        let entered = barriers.join(format!("{boundary}.entered"));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !entered.exists()
            && tokio::time::Instant::now() < deadline
            && cli.try_wait()?.is_none()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(entered.exists(), "restore did not reach {boundary}");
        assert!(cli.try_wait()?.is_none());
        let pending_keys: Value = serde_json::from_slice(&std::fs::read(&h.secrets_file)?)?;
        assert_eq!(
            pending_keys.as_object().unwrap().len(),
            original_keys.as_object().unwrap().len() + new_keys
        );
        assert_eq!(target.exists(), activated);
        assert_eq!(stages(&parent, ".restore-")?.len(), usize::from(!activated));
        assert_eq!(stages(&h.directory, ".maintenance-")?.len(), 1);
        h.force_kill().await?;
        let exit = tokio::time::timeout(Duration::from_secs(10), cli.wait()).await??;
        assert_eq!(exit.code(), Some(5));
        let lost: Value = serde_json::from_slice(&std::fs::read(&stdout)?)?;
        assert_eq!(lost["error"]["code"], "restore_outcome_uncertain");
        assert!(lost.get("result").is_none());
        h.restart().await?;
        let after: Value = serde_json::from_slice(&std::fs::read(&h.secrets_file)?)?;
        assert_eq!(
            after.as_object().unwrap().len(),
            original_keys.as_object().unwrap().len() + if activated { 2 } else { 0 },
            "new keys left behind after {boundary}"
        );
        for (name, value) in original_keys.as_object().unwrap() {
            assert!(after.get(name) == Some(value), "original key changed");
        }
        assert!(
            stages(&parent, ".restore-")?.is_empty(),
            "abandoned stage after {boundary}"
        );
        assert!(
            stages(&h.directory, ".maintenance-")?.is_empty(),
            "abandoned upload after {boundary}"
        );
        assert_eq!(
            std::fs::read(h.directory.join("profile.json"))?,
            original_profile
        );
        assert_eq!(std::fs::read(&backup)?, original_backup);
        assert_eq!(
            serde_json::to_value(h.google.control().snapshot().await)?,
            remote
        );
        let shown = result(
            h.cli(&[
                "--json",
                "mail",
                "draft",
                "show",
                "--account",
                &account,
                "--draft",
                draft,
            ])
            .await?,
        );
        assert_eq!(shown, original_draft);
        let source = h.directory.clone();
        h.shutdown().await?;
        if activated {
            h.directory = target.clone();
            h.restart().await?;
            let restored = result(
                h.cli(&[
                    "--json",
                    "mail",
                    "draft",
                    "show",
                    "--account",
                    &account,
                    "--draft",
                    draft,
                ])
                .await?,
            );
            assert_eq!(restored, original_draft);
            assert_eq!(
                serde_json::to_value(h.google.control().snapshot().await)?,
                remote
            );
            h.shutdown().await?;
        } else {
            assert!(!target.exists());
        }
        h.directory = source;
        h.restart().await?;
        assert!(serde_json::from_slice::<Value>(&std::fs::read(&h.secrets_file)?)? == after);
        assert_eq!(
            serde_json::to_value(h.google.control().snapshot().await)?,
            remote
        );
        h.shutdown().await?;
    }
    Ok(())
}
