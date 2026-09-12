#![allow(clippy::unwrap_used)]
use nuncio_test_support::{
    google::Seed,
    process::{isolated_command, E2eHarness},
};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;

async fn invoke(h: &E2eHarness, credential: &str) -> std::process::Output {
    let mut command = isolated_command(
        Path::new(env!("CARGO_BIN_EXE_nuncio-api-smoke")),
        &h.artifacts,
    );
    command
        .arg(&h.endpoint)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(credential.as_bytes())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn external_generated_client_reads_status_and_replays_authenticated_changes() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let keys: Value = serde_json::from_slice(&std::fs::read(&h.secrets_file).unwrap()).unwrap();
    let key = keys
        .as_object()
        .unwrap()
        .iter()
        .find(|(name, _)| name.ends_with("/profile/api"))
        .unwrap()
        .1
        .as_str()
        .unwrap();
    let credential = zeroize::Zeroizing::new(format!("Bearer {key}"));
    let output = invoke(&h, &credential).await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["api_version"], "nuncio.v2");
    assert!(result["profile_id"].is_string());
    assert_eq!(result["change"]["account_id"], account);
    assert!(result["change"]["revision"].as_u64().unwrap() > 0);
    assert!(result["change"]["revision"].as_u64().unwrap() <= result["revision"].as_u64().unwrap());
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(key));
    let denied = invoke(&h, "Bearer invalid").await;
    assert_eq!(denied.status.code(), Some(1));
    assert!(denied.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&denied.stderr).contains("invalid"));
    let independent = h.google.control().snapshot().await;
    assert!(independent
        .mail
        .values()
        .all(|mail| mail.accepted_sends.is_empty()));
    assert!(independent.calendars.values().all(|calendars| calendars
        .values()
        .all(|calendar| calendar.notifications.is_empty())));
    h.shutdown().await.unwrap();
}
