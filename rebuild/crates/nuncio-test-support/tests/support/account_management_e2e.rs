use nuncio_test_support::{google::Seed, process::E2eHarness};

#[tokio::test]
async fn account_archive_and_purge_survive_actual_process_death() {
    use nuncio_test_support::process::{binary, isolated_command};
    use std::{process::Stdio, time::Duration};
    for boundary in [
        "account_before_archive",
        "account_after_archive",
        "account_before_purge",
        "account_after_purge",
    ] {
        let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
        let alpha = h.connect_google("alpha@example.test").await.unwrap();
        let beta = h.connect_google("beta@example.test").await.unwrap();
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &alpha, "--wait"])
                .await
                .unwrap()
                .status,
            0
        );
        let purge = boundary.contains("purge");
        if purge {
            assert_eq!(
                h.cli(&["--json", "account", "remove", "--account", &alpha])
                    .await
                    .unwrap()
                    .status,
                0
            );
        }
        let remote = serde_json::to_value(h.google.control().snapshot().await).unwrap();
        let barriers = h.artifacts.join("barriers");
        std::fs::write(barriers.join(format!("{boundary}.arm")), []).unwrap();
        let mut command =
            isolated_command(&binary("NUNCIO_E2E_CLI").unwrap(), &h.artifacts.join("tmp"));
        command
            .arg("--endpoint")
            .arg(&h.endpoint)
            .arg("--data-dir")
            .arg(&h.directory)
            .arg("--test-secrets-file")
            .arg(&h.secrets_file)
            .args([
                "--json",
                "account",
                if purge { "purge" } else { "remove" },
                "--account",
                &alpha,
            ]);
        if purge {
            command.args(["--confirm", &alpha]);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(h.artifacts.join("account-crash.stdout.log")).unwrap())
            .stderr(std::fs::File::create(h.artifacts.join("account-crash.stderr.log")).unwrap())
            .spawn()
            .unwrap();
        let entered = barriers.join(format!("{boundary}.entered"));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !entered.exists() && tokio::time::Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            entered.exists(),
            "account mutation did not reach {boundary}"
        );
        h.force_kill().await.unwrap();
        assert!(!tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        h.restart().await.unwrap();
        let shown = h
            .cli(&["--json", "account", "show", "--account", &alpha])
            .await
            .unwrap();
        if boundary == "account_after_purge" {
            assert_eq!(shown.status, 4);
        } else {
            assert_eq!(shown.status, 0);
            let state = if boundary == "account_before_archive" {
                "connected"
            } else {
                "archived"
            };
            assert_eq!(shown.json().unwrap()["result"]["account"]["state"], state);
            let mail = h
                .cli(&["--json", "mail", "list", "--account", &alpha])
                .await
                .unwrap()
                .json()
                .unwrap();
            assert_eq!(mail["result"]["items"].as_array().unwrap().len(), 3);
        }
        let other = h
            .cli(&["--json", "account", "show", "--account", &beta])
            .await
            .unwrap();
        assert_eq!(other.status, 0);
        assert_eq!(
            other.json().unwrap()["result"]["account"]["state"],
            "connected"
        );
        assert_eq!(
            serde_json::to_value(h.google.control().snapshot().await).unwrap(),
            remote
        );
        std::fs::write(h.artifacts.join("account-crash-evidence.json"), serde_json::to_vec_pretty(&serde_json::json!({"boundary":boundary,"daemon_killed":true,"restart_verified":true,"remote_snapshot_unchanged":true,"other_account_retained":true})).unwrap()).unwrap();
        h.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn google_auth_wait_cancel_and_archive_cannot_revive_saved_accounts() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let registration = h.artifacts.join("mock-client-registration.json");
    let begin = h
        .cli(&[
            "--json",
            "account",
            "reauth-google",
            "--account",
            &account,
            "--client-config",
            registration.to_str().unwrap(),
            "--no-browser",
            "--no-wait",
        ])
        .await
        .unwrap();
    assert_eq!(begin.status, 0, "reauth must target the saved identity");
    let begin = begin.json().unwrap()["result"].clone();
    let session = begin["session_id"].as_str().unwrap();
    let waiting = h
        .cli(&[
            "--json",
            "account",
            "auth-wait",
            "--session",
            session,
            "--timeout-seconds",
            "1",
        ])
        .await
        .unwrap();
    assert_ne!(waiting.status, 0, "pending browser consent is not success");
    assert_eq!(waiting.json().unwrap()["error"]["code"], "auth_timeout");
    let cancel = h
        .cli(&["--json", "account", "auth-cancel", "--session", session])
        .await
        .unwrap();
    assert_eq!(cancel.status, 0);
    assert_eq!(cancel.json().unwrap()["result"]["state"], "cancelled");
    assert_ne!(
        h.cli(&["--json", "account", "auth-wait", "--session", session])
            .await
            .unwrap()
            .status,
        0
    );
    let start = h
        .cli(&[
            "--json",
            "account",
            "connect-google",
            "--client-config",
            registration.to_str().unwrap(),
            "--login-hint",
            "alpha@example.test",
            "--no-browser",
        ])
        .await
        .unwrap();
    assert_eq!(start.status, 0);
    let start = start.json().unwrap()["result"].clone();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let consent = client
        .get(start["browser_url"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(consent.status(), 302);
    let callback = consent
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        h.cli(&["--json", "account", "remove", "--account", &account])
            .await
            .unwrap()
            .status,
        0
    );
    let cancelled = h
        .cli(&[
            "--json",
            "account",
            "auth-status",
            "--session",
            start["session_id"].as_str().unwrap(),
        ])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(cancelled["result"]["state"], "cancelled");
    if let Ok(response) = client.get(callback).send().await {
        assert!(!response.status().is_success());
    }
    assert_eq!(
        h.cli(&["--json", "account", "restore", "--account", &account])
            .await
            .unwrap()
            .status,
        0
    );
    let restored = h
        .cli(&["--json", "account", "show", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(restored["result"]["account"]["auth_state"], "disconnected");
    let started = h
        .cli(&[
            "--json",
            "account",
            "add-google",
            "--client-config",
            registration.to_str().unwrap(),
            "--login-hint",
            "alpha@example.test",
            "--no-browser",
            "--no-wait",
        ])
        .await
        .unwrap();
    assert_eq!(started.status, 0);
    let started = started.json().unwrap()["result"].clone();
    let response = client
        .get(started["browser_url"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(client.get(location).send().await.unwrap().status(), 200);
    let success = h
        .cli(&[
            "--json",
            "account",
            "auth-wait",
            "--session",
            started["session_id"].as_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(success.status, 0);
    assert_eq!(success.json().unwrap()["result"]["account_id"], account);
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
    h.google.stop().await.unwrap();
}

#[tokio::test]
async fn account_details_edit_pause_archive_restore_and_purge_work_through_cli() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let alpha = h.connect_google("alpha@example.test").await.unwrap();
    let beta = h.connect_google("beta@example.test").await.unwrap();
    for id in [&alpha, &beta] {
        assert_eq!(
            h.cli(&["--json", "sync", "--account", id, "--wait"])
                .await
                .unwrap()
                .status,
            0
        );
    }
    let shown = h
        .cli(&["--json", "account", "show", "--account", &alpha])
        .await
        .unwrap();
    assert_eq!(
        shown.status, 0,
        "the actual CLI must expose saved account details"
    );
    let shown = shown.json().unwrap();
    assert_eq!(shown["result"]["account"]["address"], "alpha@example.test");
    let version = shown["result"]["account"]["version"]
        .as_u64()
        .unwrap()
        .to_string();
    let edited = h
        .cli(&[
            "--json",
            "account",
            "edit",
            "--account",
            &alpha,
            "--name",
            "Personal",
            "--version",
            &version,
        ])
        .await
        .unwrap();
    assert_eq!(edited.status, 0);
    assert_eq!(edited.json().unwrap()["result"]["display_name"], "Personal");
    assert_eq!(
        h.cli(&[
            "--json",
            "account",
            "edit",
            "--account",
            &alpha,
            "--name",
            "Stale edit",
            "--version",
            &version
        ])
        .await
        .unwrap()
        .status,
        5
    );
    let remote_before = serde_json::to_value(h.google.control().snapshot().await.requests).unwrap();
    assert_eq!(
        h.cli(&["--json", "account", "pause", "--account", &alpha])
            .await
            .unwrap()
            .status,
        0
    );
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let paused = h
        .cli(&["--json", "account", "show", "--account", &alpha])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(paused["result"]["account"]["state"], "paused");
    assert_ne!(
        h.cli(&["--json", "sync", "--account", &alpha, "--wait"])
            .await
            .unwrap()
            .status,
        0
    );
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await.requests).unwrap(),
        remote_before
    );
    assert_eq!(
        h.cli(&["--json", "account", "resume", "--account", &alpha])
            .await
            .unwrap()
            .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "account", "remove", "--account", &alpha])
            .await
            .unwrap()
            .status,
        0
    );
    let listed = h
        .cli(&["--json", "account", "list"])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(listed["result"]["accounts"].as_array().unwrap().len(), 1);
    let all = h
        .cli(&["--json", "account", "list", "--include-archived"])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(all["result"]["accounts"].as_array().unwrap().len(), 2);
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(
        h.cli(&["--json", "account", "restore", "--account", &alpha])
            .await
            .unwrap()
            .status,
        0
    );
    let restored = h
        .cli(&["--json", "account", "show", "--account", &alpha])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(restored["result"]["account"]["state"], "disconnected");
    assert_eq!(restored["result"]["account"]["display_name"], "Personal");
    assert_eq!(
        h.cli(&["--json", "account", "remove", "--account", &alpha])
            .await
            .unwrap()
            .status,
        0
    );
    let preview = h
        .cli(&[
            "--json",
            "account",
            "purge",
            "--account",
            &alpha,
            "--dry-run",
        ])
        .await
        .unwrap();
    assert_eq!(preview.status, 0);
    assert_eq!(preview.json().unwrap()["result"]["messages"], 3);
    assert_ne!(
        h.cli(&[
            "--json",
            "account",
            "purge",
            "--account",
            &alpha,
            "--confirm",
            &beta
        ])
        .await
        .unwrap()
        .status,
        0
    );
    assert_eq!(
        h.cli(&[
            "--json",
            "account",
            "purge",
            "--account",
            &alpha,
            "--confirm",
            &alpha
        ])
        .await
        .unwrap()
        .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "account", "show", "--account", &alpha])
            .await
            .unwrap()
            .status,
        4
    );
    let retained = h
        .cli(&["--json", "mail", "list", "--account", &beta])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(retained["result"]["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await.requests).unwrap(),
        remote_before,
        "local account lifecycle must not change provider state or contact it"
    );
    h.shutdown().await.unwrap();
    h.google.stop().await.unwrap();
}
