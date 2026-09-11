#![cfg(feature = "test-harness")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
};

async fn start(directory: &Path, secrets: &Path) -> (Child, String) {
    let ready_file = secrets.with_extension("ready.json");
    if ready_file.exists() {
        std::fs::remove_file(&ready_file).unwrap();
    }
    let daemon = std::env::var_os("NUNCIO_E2E_DAEMON")
        .expect("Set NUNCIO_E2E_DAEMON to the test-harness daemon binary");
    let mut child = Command::new(daemon)
        .env_clear()
        .args(["--bind", "127.0.0.1:0", "--data-dir"])
        .arg(directory)
        .arg("--test-secrets-file")
        .arg(secrets)
        .arg("--ready-file")
        .arg(&ready_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let bytes = tokio::time::timeout(Duration::from_secs(10), output.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(
        bytes > 0,
        "actual daemon must initialize and report readiness with a mock keystore"
    );
    let ready: serde_json::Value = serde_json::from_str(&line).unwrap();
    let file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ready_file).unwrap()).unwrap();
    assert_eq!(ready, file);
    (child, ready["endpoint"].as_str().unwrap().to_owned())
}

async fn cli(
    directory: &Path,
    secrets: &Path,
    endpoint: &str,
    action: &str,
) -> std::process::Output {
    tokio::time::timeout(
        Duration::from_secs(5),
        Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
            .env_clear()
            .arg("--json")
            .arg("--data-dir")
            .arg(directory)
            .arg("--test-secrets-file")
            .arg(secrets)
            .args(["--endpoint", endpoint, "system", action])
            .output(),
    )
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn actual_cli_authenticates_and_restarts_actual_daemon_without_os_keyring() {
    let temporary = tempfile::tempdir().unwrap();
    let directory: PathBuf = temporary.path().join("profile");
    let secrets = temporary.path().join("synthetic-secrets.json");
    let (mut child, endpoint) = start(&directory, &secrets).await;
    let status = cli(&directory, &secrets, &endpoint, "status").await;
    assert_eq!(status.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let id = value["result"]["profile_id"].as_str().unwrap().to_owned();
    assert_eq!(value["result"]["api_version"], "nuncio.v2");
    assert!(!String::from_utf8_lossy(&status.stdout).contains("Bearer"));
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&secrets).unwrap()).unwrap();
    wrong[format!("{id}/profile/api")] = serde_json::Value::String("00".repeat(32));
    let wrong_path = temporary.path().join("wrong-secrets.json");
    std::fs::write(&wrong_path, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert_eq!(
        cli(&directory, &wrong_path, &endpoint, "status")
            .await
            .status
            .code(),
        Some(3)
    );
    assert!(cli(&directory, &secrets, &endpoint, "shutdown")
        .await
        .status
        .success());
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    assert_eq!(
        cli(&directory, &secrets, &endpoint, "status")
            .await
            .status
            .code(),
        Some(4)
    );
    let (mut child, endpoint) = start(&directory, &secrets).await;
    let status = cli(&directory, &secrets, &endpoint, "status").await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout).unwrap()["result"]
            ["profile_id"],
        id
    );
    assert!(cli(&directory, &secrets, &endpoint, "shutdown")
        .await
        .status
        .success());
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
