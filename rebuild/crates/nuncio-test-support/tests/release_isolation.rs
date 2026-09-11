#![allow(clippy::unwrap_used)]
use nuncio_test_support::process::{binary, isolated_command};
use std::time::Duration;

#[test]
fn migration_checkpoints_are_present_only_in_the_feature_daemon() {
    let harness = std::fs::read(binary("NUNCIO_E2E_DAEMON").unwrap()).unwrap();
    for marker in [
        b"-before-commit".as_slice(),
        b"-after-commit".as_slice(),
        b"reconciliation_after_admission".as_slice(),
        b"restore_stage_owned".as_slice(),
        b"restore_after_stage".as_slice(),
        b"restore_after_database_key".as_slice(),
        b"restore_after_api_key".as_slice(),
        b"restore_after_activation".as_slice(),
    ] {
        assert!(harness.windows(marker.len()).any(|part| part == marker));
        for variable in ["NUNCIO_RELEASE_DAEMON", "NUNCIO_RELEASE_CLI"] {
            let release = std::fs::read(binary(variable).unwrap()).unwrap();
            assert!(!release.windows(marker.len()).any(|part| part == marker));
        }
    }
}

#[tokio::test]
async fn production_binaries_reject_test_flags_and_environment_before_profile_access() {
    let directory = tempfile::tempdir().unwrap();
    for (variable, is_daemon) in [
        ("NUNCIO_RELEASE_DAEMON", true),
        ("NUNCIO_RELEASE_CLI", false),
    ] {
        let binary = binary(variable).unwrap();
        for flag in [
            "--test-secrets-file",
            "--test-config",
            "--test-google-base-url",
            "--test-clock-ms",
            "--test-barrier",
        ] {
            let output = tokio::time::timeout(
                Duration::from_secs(5),
                isolated_command(&binary, directory.path())
                    .args([flag, "unused"])
                    .output(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                output.status.code(),
                Some(2),
                "production must reject {flag}"
            );
        }
        for name in [
            "NUNCIO_TEST_SECRETS_FILE",
            "NUNCIO_TEST_GOOGLE_BASE_URL",
            "NUNCIO_TEST_CLOCK_MS",
            "NUNCIO_TEST_BARRIER",
        ] {
            let profile = directory.path().join("must-not-create");
            let mut command = isolated_command(&binary, directory.path());
            command
                .arg("--data-dir")
                .arg(&profile)
                .env(name, "synthetic-test-value");
            if !is_daemon {
                command.args([
                    "--json",
                    "--endpoint",
                    "http://127.0.0.1:1",
                    "system",
                    "status",
                ]);
            }
            let output = tokio::time::timeout(Duration::from_secs(5), command.output())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(output.status.code(), Some(if is_daemon { 1 } else { 2 }));
            assert!(!profile.exists());
        }
    }
}
