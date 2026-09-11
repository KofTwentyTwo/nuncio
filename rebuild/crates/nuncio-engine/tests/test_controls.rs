#![cfg(feature = "test-harness")]
#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    engine::{Engine, EngineConfig},
    secrets::{keyring::OsKeyring, test_store::FileTestStore},
    test_controls::TestConfig,
};
use std::sync::Arc;

#[tokio::test]
async fn test_configuration_rejects_remote_endpoints_and_real_keystores_before_creating_data() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("profile");
    for base in [
        "https://gmail.googleapis.com",
        "http://localhost:8080",
        "http://127.0.0.1.evil.test",
        "http://user@127.0.0.1:8",
        "http://127.0.0.1:8/path",
    ] {
        let settings = TestConfig::google(base);
        assert!(settings.validate().is_err());
        assert!(Engine::open_for_test(
            EngineConfig {
                directory: directory.clone(),
                secrets: Arc::new(FileTestStore::new(temp.path().join("keys.json")))
            },
            settings
        )
        .await
        .is_err());
        assert!(!directory.exists());
    }
    let settings = TestConfig::google("http://127.0.0.1:8089");
    assert!(Engine::open_for_test(
        EngineConfig {
            directory: directory.clone(),
            secrets: Arc::new(OsKeyring)
        },
        settings.clone()
    )
    .await
    .is_err());
    assert!(!directory.exists());
    let engine = Engine::open_for_test(
        EngineConfig {
            directory,
            secrets: Arc::new(FileTestStore::new(temp.path().join("keys.json"))),
        },
        settings,
    )
    .await
    .unwrap();
    assert_eq!(
        engine.test_config().unwrap().google_base_url,
        "http://127.0.0.1:8089"
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_barriers_are_armed_out_of_band_and_cancellable() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = TestConfig::google("http://127.0.0.1:8089");
    settings.barriers_directory = Some(temp.path().into());
    settings.now_unix_ms = Some(1772895600000);
    assert_eq!(settings.now_ms().unwrap(), 1772895600000);
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let config = settings.clone();
    std::fs::write(temp.path().join("before-send.arm"), b"").unwrap();
    let task = tokio::spawn(async move { config.checkpoint("before-send", receiver).await });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(5));
    while !temp.path().join("before-send.entered").exists() {
        assert!(tokio::time::Instant::now() < deadline);
        poll.tick().await;
    }
    assert!(!task.is_finished());
    std::fs::write(temp.path().join("before-send.release"), b"").unwrap();
    task.await.unwrap().unwrap();
    std::fs::write(temp.path().join("after-send.arm"), b"").unwrap();
    let config = settings.clone();
    let receiver = stop.subscribe();
    let task = tokio::spawn(async move { config.checkpoint("after-send", receiver).await });
    stop.send_replace(true);
    assert!(task.await.unwrap().is_err());
    assert!(settings
        .checkpoint("../escape", stop.subscribe())
        .await
        .is_err());
}
