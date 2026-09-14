use nuncio_test_support::{google::Seed, process::E2eHarness, TestError};
use serde_json::json;
#[path = "terminal.rs"]
mod terminal;

#[tokio::test]
async fn guided_google_setup_uses_browser_consent_and_survives_restart() -> Result<(), TestError> {
    google_setup(false).await
}

#[tokio::test]
async fn guided_google_setup_uses_bundled_registration_without_a_json_file() -> Result<(), TestError>
{
    google_setup(true).await
}

async fn google_setup(managed: bool) -> Result<(), TestError> {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let registration = h.artifacts.join("wizard-google.json");
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&registration)?
            .write_all(br#"{"installed":{"client_id":"nuncio-test-client"}}"#)?;
    }
    let before = h.google.control().snapshot().await;
    let args = if managed {
        vec!["account", "add", "--no-browser"]
    } else {
        vec![
            "account",
            "add",
            "--client-config",
            registration.to_str().ok_or("path")?,
            "--no-browser",
        ]
    };
    if managed {
        std::fs::remove_file(&registration)?;
    }
    let result = terminal::run_with_binary(&h, &args, json!([
        {"prompt":"Provider", "answer":"1"}, {"prompt":"Email address", "answer":"alpha@example.test"},
        {"prompt":"Connect and start syncing?", "answer":"yes"}
    ]), Some(h.google.base_url()), if managed {"NUNCIO_E2E_MANAGED_CLI"} else {"NUNCIO_E2E_CLI"}).await?;
    assert_eq!(result["exit_status"], 0);
    assert_eq!(result["echo_restored"], true);
    assert!(result["transcript"]
        .as_str()
        .unwrap()
        .contains("Account connected"));
    let list = h.cli(&["--json", "account", "list"]).await?.json()?;
    assert_eq!(list["result"]["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(
        list["result"]["accounts"][0]["address"],
        "alpha@example.test"
    );
    let id = list["result"]["accounts"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    h.force_kill().await?;
    h.restart().await?;
    assert_eq!(
        h.cli(&["account", "check", "--account", &id]).await?.status,
        0
    );
    let after = h.google.control().snapshot().await;
    assert_eq!(
        serde_json::to_value(before.mail)?,
        serde_json::to_value(after.mail)?
    );
    assert_eq!(
        serde_json::to_value(before.calendars)?,
        serde_json::to_value(after.calendars)?
    );
    h.shutdown().await?;
    h.google.stop().await?;
    Ok(())
}

#[tokio::test]
async fn guided_setup_requires_terminal_and_cancel_does_not_add_account() -> Result<(), TestError> {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let piped = h.cli(&["--json", "account", "add"]).await?;
    assert_eq!(piped.status, 2);
    assert_eq!(
        piped.json()?["error"]["code"],
        "interactive_terminal_required"
    );
    let result = terminal::run(&h, &["account", "add"], json!([
        {"prompt":"Provider", "answer":"2"}, {"prompt":"Email address", "answer":"alpha@example.test"},
        {"prompt":"Mail server", "answer":"mail.example.invalid"}, {"prompt":"Username", "answer":""},
        {"prompt":"Password", "secret":true, "interrupt":true}
    ]), None).await?;
    assert_eq!(result["exit_status"], 130);
    assert_eq!(result["echo_restored"], true);
    assert_eq!(
        h.cli(&["--json", "account", "list"]).await?.json()?["result"]["accounts"],
        json!([])
    );
    h.shutdown().await?;
    h.google.stop().await?;
    Ok(())
}

#[tokio::test]
async fn guided_setup_missing_registration_and_declined_consent_are_inert() -> Result<(), TestError>
{
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let before = h.google.control().snapshot().await;
    let missing = terminal::run(
        &h,
        &["account", "add"],
        json!([
            {"prompt":"Provider", "answer":"1"}
        ]),
        None,
    )
    .await?;
    assert_eq!(missing["exit_status"], 2);
    let transcript = missing["transcript"].as_str().unwrap();
    assert!(transcript.contains("Google sign-in needs an app registration"));
    assert!(transcript.contains("--client-config"));
    assert!(transcript.contains("docs/GOOGLE-SETUP.md"));
    assert!(!transcript.contains("Email address"));
    let declined = terminal::run(&h, &["account", "add"], json!([
        {"prompt":"Provider", "answer":"invalid"}, {"prompt":"Provider", "answer":"2"},
        {"prompt":"Email address", "answer":"invalid"}, {"prompt":"Email address", "answer":"alpha@example.test"},
        {"prompt":"Mail server", "answer":"https://example.test"}, {"prompt":"Mail server", "answer":"mail.example.invalid"},
        {"prompt":"Username", "answer":""}, {"prompt":"Password", "secret":true, "answer":"synthetic-wizard-never-send"},
        {"prompt":"Change advanced server settings?", "answer":""}, {"prompt":"Connect and start syncing?", "answer":"no"}
    ]), None).await?;
    assert_eq!(declined["exit_status"], 130);
    assert_eq!(declined["secrets_hidden"], 1);
    assert_eq!(declined["echo_restored"], true);
    assert_eq!(
        h.cli(&["--json", "account", "list"]).await?.json()?["result"]["accounts"],
        json!([])
    );
    let after = h.google.control().snapshot().await;
    assert_eq!(serde_json::to_value(before)?, serde_json::to_value(after)?);
    h.shutdown().await?;
    h.google.stop().await?;
    Ok(())
}

#[tokio::test]
async fn guided_google_cancel_rejects_a_late_browser_callback() -> Result<(), TestError> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let registration = h.artifacts.join("cancel-google.json");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&registration)?
        .write_all(br#"{"installed":{"client_id":"nuncio-test-client"}}"#)?;
    let before = h.google.control().snapshot().await;
    let result = terminal::run(&h, &["account", "add", "--client-config", registration.to_str().unwrap(), "--no-browser"], json!([
        {"prompt":"Provider", "answer":"1"}, {"prompt":"Email address", "answer":"alpha@example.test"},
        {"prompt":"Connect and start syncing?", "answer":"yes"},
        {"prompt":"Complete Google consent in your browser:", "interrupt":true}
    ]), None).await?;
    assert_eq!(result["exit_status"], 5, "{result}");
    assert_eq!(result["echo_restored"], true);
    assert!(result["transcript"]
        .as_str()
        .unwrap()
        .contains("Google authorization was cancelled"));
    let url = result["transcript"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .find(|part| part.starts_with("http://"))
        .ok_or("missing consent URL")?;
    assert!(url.starts_with(&format!("{}/", h.google.base_url())));
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let response = http.get(url).send().await?;
    assert_eq!(response.status(), 302);
    let callback = response.headers()["location"].to_str()?;
    assert!(callback.starts_with("http://127.0.0.1:"));
    if let Ok(response) = http.get(callback).send().await {
        assert!(
            !response.status().is_success(),
            "cancelled consent must not admit an account"
        );
    }
    assert_eq!(
        h.cli(&["--json", "account", "list"]).await?.json()?["result"]["accounts"],
        json!([])
    );
    let after = h.google.control().snapshot().await;
    assert_eq!(
        serde_json::to_value(before.mail)?,
        serde_json::to_value(after.mail)?
    );
    assert_eq!(
        serde_json::to_value(before.calendars)?,
        serde_json::to_value(after.calendars)?
    );
    h.shutdown().await?;
    h.google.stop().await?;
    Ok(())
}
