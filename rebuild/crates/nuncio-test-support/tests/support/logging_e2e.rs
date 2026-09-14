use super::{E2eHarness, Seed};

#[tokio::test]
async fn daemon_default_info_reports_real_sync_and_shutdown_without_private_data() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let synced = h
        .cli(&["--json", "sync", "--account", &account, "--wait"])
        .await
        .unwrap();
    assert_eq!(synced.status, 0);
    let run = synced.json().unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await
        .unwrap();
    assert_eq!(
        listed.json().unwrap()["result"]["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(!h.google.control().snapshot().await.requests.is_empty());
    assert_eq!(
        h.cli(&[
            "--json",
            "calendar",
            "refresh",
            "--account",
            &account,
            "--from",
            "2026-10-01",
            "--to",
            "2026-10-05",
            "--wait"
        ])
        .await
        .unwrap()
        .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "account", "pause", "--account", &account])
            .await
            .unwrap()
            .status,
        0
    );
    h.shutdown().await.unwrap();
    // Read ORIGINAL process output before the harness Drop redactor touches it.
    let logs = std::fs::read_to_string(h.artifacts.join("daemon-1.stderr.log")).unwrap();
    for event in [
        "Daemon starting",
        "Daemon ready",
        "Account connected",
        "Account lifecycle changed",
        "Sync queued",
        "Sync started",
        "Sync progress",
        "Sync completed",
        "Shutdown requested",
        "Daemon stopped",
    ] {
        assert!(logs.contains(event), "missing event: {event}");
    }
    assert!(logs.contains(&account));
    assert!(logs.contains(&run));
    assert!(logs.contains("scope=calendar"));
    assert!(logs.contains("scope=gmail"));
    assert!(logs.contains("INFO"));
    assert!(!logs.contains("DEBUG"));
    assert_private_data_absent(&h, &logs);
    let stdout = std::fs::read_to_string(h.artifacts.join("daemon-1.stdout.log")).unwrap();
    assert_eq!(
        stdout.lines().count(),
        1,
        "readiness stdout remains one JSON object"
    );
    let ready: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(ready["event"], "ready");
    assert_eq!(ready["endpoint"], h.endpoint);
}

fn assert_private_data_absent(h: &E2eHarness, logs: &str) {
    for forbidden in [
        "alpha@example.test",
        "beta@example.test",
        "Multipart fixture",
        "searchable",
        "Bearer",
        "mock-access-",
        "mock-refresh-",
        "client_secret",
        "refresh_token",
    ] {
        assert!(
            !logs.contains(forbidden),
            "daemon emitted private fixture data"
        );
    }
    let secrets: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(&h.secrets_file).unwrap()).unwrap();
    assert!(!secrets.is_empty());
    for value in secrets.values() {
        assert!(
            !logs.contains(value),
            "daemon emitted a stored synthetic secret"
        );
        let decoded = hex::decode(value).unwrap();
        if let Ok(text) = std::str::from_utf8(&decoded) {
            assert!(
                !logs.contains(text),
                "daemon emitted a decoded synthetic secret"
            );
        }
    }
}

#[tokio::test]
async fn daemon_debug_reports_uncertain_send_and_reconciliation_with_one_remote_send() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    h.shutdown().await.unwrap();
    h.restart_with_log_level(Some("debug")).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let draft_path = h.artifacts.join("private-draft.json");
    std::fs::write(&draft_path, br#"{"to":[{"address":"recipient@example.test"}],"subject":"logging-subject-canary","text":"logging-body-canary"}"#).unwrap();
    let draft = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            draft_path.to_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(draft.status, 0);
    let draft = draft.json().unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.google
        .control()
        .inject(Fault {
            method: "POST".into(),
            path: "/gmail/v1/users/me/messages/send".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::After,
            action: FaultAction::Withhold {
                barrier: "logging-lost-ack".into(),
            },
        })
        .await;
    let sent = h
        .cli(&[
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "c09cc5d8-39c4-4510-9989-9f8ff9a9b533",
            "--wait",
        ])
        .await
        .unwrap();
    assert_eq!(
        sent.status, 0,
        "lost acknowledgement must reconcile to applied"
    );
    let operation = sent.json().unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(sent.json().unwrap()["result"]["state"], "applied");
    assert_eq!(
        h.google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .len(),
        1,
        "logging must not hide duplicate remote sends after lost acknowledgements"
    );
    h.shutdown().await.unwrap();
    let logs = std::fs::read_to_string(h.artifacts.join("daemon-2.stderr.log")).unwrap();
    assert!(logs.contains("DEBUG"));
    for expected in [
        "Operation attempt started",
        "Operation attempt finished",
        "state=uncertain",
        "state=applied",
        "kind=reconcile",
        &operation,
    ] {
        assert!(logs.contains(expected), "missing operation log: {expected}");
    }
    assert!(logs
        .lines()
        .any(|line| line.contains("WARN") && line.contains("state=uncertain")));
    for forbidden in [
        "logging-subject-canary",
        "logging-body-canary",
        "recipient@example.test",
    ] {
        assert!(!logs.contains(forbidden));
    }
    assert_private_data_absent(&h, &logs);
}

#[tokio::test]
async fn daemon_warn_and_off_filter_info_but_keep_readiness_and_safe_failures() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for level in ["warn", "off", "trace"] {
        let generation = 2;
        let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
        let account = h.connect_google("alpha@example.test").await.unwrap();
        h.shutdown().await.unwrap();
        h.restart_with_log_level(Some(level)).await.unwrap();
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--full", "--wait"])
                .await
                .unwrap()
                .status,
            0
        );
        h.google
            .control()
            .inject(Fault {
                method: "GET".into(),
                path: "/gmail/v1/users/me/profile".into(),
                account: Some("alpha@example.test".into()),
                call: None,
                phase: Phase::Before,
                action: FaultAction::Status {
                    code: 403,
                    retry_after_secs: None,
                },
            })
            .await;
        let sync = h
            .cli(&["--json", "sync", "--account", &account, "--full", "--wait"])
            .await
            .unwrap();
        assert_ne!(sync.status, 0);
        h.shutdown().await.unwrap();
        let logs =
            std::fs::read_to_string(h.artifacts.join(format!("daemon-{generation}.stderr.log")))
                .unwrap();
        if level == "off" {
            assert!(logs.is_empty());
        } else {
            assert!(logs.contains("WARN"));
            assert!(logs.contains("Sync stopped"));
            assert!(logs.contains("state=failed"));
        }
        if level == "warn" {
            assert!(!logs.contains("INFO"));
            assert!(!logs.contains("DEBUG"));
        }
        if level == "trace" {
            assert!(logs.contains("DEBUG"));
        }
        assert_private_data_absent(&h, &logs);
        let ready: serde_json::Value = serde_json::from_slice(
            &std::fs::read(h.artifacts.join(format!("daemon-{generation}.stdout.log"))).unwrap(),
        )
        .unwrap();
        assert_eq!(ready["event"], "ready");
    }
}

#[tokio::test]
async fn daemon_startup_reports_ordered_phases_for_new_and_existing_profiles() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    assert_eq!(
        h.cli(&["--json", "account", "pause", "--account", &account])
            .await
            .unwrap()
            .status,
        0
    );
    h.shutdown().await.unwrap();
    let before = serde_json::to_value(h.google.control().snapshot().await).unwrap();
    h.restart().await.unwrap();
    assert_eq!(
        h.cli(&["--json", "system", "status"]).await.unwrap().status,
        0
    );
    h.shutdown().await.unwrap();
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await).unwrap(),
        before
    );
    for generation in [1, 2] {
        let logs =
            std::fs::read_to_string(h.artifacts.join(format!("daemon-{generation}.stderr.log")))
                .unwrap();
        let mut after = 0;
        for phase in [
            "Daemon starting",
            "Validating startup configuration",
            "Local API listener bound",
            "Preparing local profile",
            "Profile lock acquired",
            "Profile metadata ready",
            "Accessing credential store for profile keys",
            "Profile keys ready",
            "Opening encrypted database",
            "Database encryption verified",
            "Checking database schema",
            "Database schema ready",
            "Checking interrupted restore jobs",
            "Recovering local drafts and durable operations",
            "Loading account state and credential cleanup",
            "Recovery checks complete",
            "Starting background workers",
            "Background workers started",
            "Publishing daemon readiness",
            "Daemon ready",
        ] {
            let offset = logs[after..].find(phase);
            assert!(
                offset.is_some(),
                "missing or out-of-order startup phase: {phase}"
            );
            after += offset.unwrap() + phase.len();
        }
        assert!(logs.contains("schema_version=23"));
        assert!(logs.contains("startup_ms="));
        let ready_line = logs
            .lines()
            .find(|line| line.contains("Daemon ready"))
            .unwrap();
        assert!(ready_line.contains(if generation == 1 {
            "accounts=0"
        } else {
            "accounts=1"
        }));
        assert!(logs.contains(if generation == 1 {
            "current_schema=0"
        } else {
            "current_schema=23"
        }));
        assert_private_data_absent(&h, &logs);
        assert!(!logs.contains(h.directory.to_str().unwrap()));
        let stdout =
            std::fs::read_to_string(h.artifacts.join(format!("daemon-{generation}.stdout.log")))
                .unwrap();
        assert_eq!(stdout.lines().count(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stdout).unwrap()["event"],
            "ready"
        );
    }
}

#[tokio::test]
async fn daemon_startup_failure_reports_last_phase_without_false_readiness_or_provider_effects() {
    use nuncio_test_support::process::{binary, isolated_command};
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let before = serde_json::to_value(h.google.control().snapshot().await).unwrap();
    let ready = h.artifacts.join("second-instance.ready.json");
    let daemon = binary("NUNCIO_E2E_DAEMON").unwrap();
    let mut command = isolated_command(&daemon, &h.artifacts.join("tmp"));
    command
        .args(["--bind", "127.0.0.1:0", "--data-dir"])
        .arg(&h.directory)
        .arg("--test-secrets-file")
        .arg(&h.secrets_file)
        .arg("--test-config")
        .arg(h.artifacts.join("daemon-1.test.json"))
        .arg("--ready-file")
        .arg(&ready);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!ready.exists());
    let logs = String::from_utf8(output.stderr).unwrap();
    std::fs::write(h.artifacts.join("second-instance.stderr.log"), &logs).unwrap();
    assert!(logs.contains("Preparing local profile"));
    assert!(logs.contains("already in use"));
    assert!(!logs.contains("Profile lock acquired"));
    assert!(!logs.contains("Accessing credential store"));
    assert!(!logs.contains("Daemon ready"));
    assert_private_data_absent(&h, &logs);
    assert_eq!(
        h.cli(&["--json", "system", "status"]).await.unwrap().status,
        0
    );
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await).unwrap(),
        before
    );
    h.shutdown().await.unwrap();
}
