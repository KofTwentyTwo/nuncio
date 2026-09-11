#![allow(clippy::unwrap_used, clippy::expect_used)]
use nuncio_test_support::{google::Seed, process::E2eHarness};
#[path = "support/calendar_write_e2e.rs"]
mod calendar_write_e2e;
#[path = "support/draft_e2e.rs"]
mod draft_e2e;
#[path = "support/mail_change_e2e.rs"]
mod mail_change_e2e;
#[path = "support/scheduling_e2e.rs"]
mod scheduling_e2e;
#[path = "support/send_crash_e2e.rs"]
mod send_crash_e2e;
#[path = "support/send_e2e.rs"]
mod send_e2e;

#[tokio::test]
async fn gmail_initial_sync_queries_and_original_downloads_work_through_cli() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let synced = h
        .cli(&["--json", "sync", "--account", &account, "--wait"])
        .await
        .unwrap();
    assert_eq!(
        synced.status, 0,
        "actual CLI must ingest Gmail from the independent HTTP provider"
    );
    let beta = h.connect_google("beta@example.test").await.unwrap();
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &beta, "--wait"])
            .await
            .unwrap()
            .status,
        0
    );
    let remote_requests = h.google.control().snapshot().await.requests;
    h.google.stop().await.unwrap();
    assert!(
        browser_for_offline_check()
            .get(format!("{}/gmail/v1/users/me/profile", h.google.base_url()))
            .send()
            .await
            .is_err(),
        "mock listener must actually be unavailable"
    );
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let output = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await
        .unwrap();
    assert_eq!(output.status, 0);
    let list = output.json().unwrap();
    assert_eq!(list["result"]["items"].as_array().unwrap().len(), 3);
    let message = list["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["provider_id"] == "m-001")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let read = h
        .cli(&[
            "--json",
            "mail",
            "read",
            "--account",
            &account,
            "--message",
            &message,
        ])
        .await
        .unwrap();
    assert_eq!(read.status, 0);
    let read = read.json().unwrap();
    assert_eq!(read["result"]["message"]["subject"], "Multipart fixture");
    assert_eq!(read["result"]["message"]["body_availability"], "available");
    let attachment = read["result"]["attachments"][0]["id"].as_str().unwrap();
    assert!(read["result"]["text"]
        .as_str()
        .unwrap()
        .contains("searchable"));
    assert!(read["result"]["html"].is_null());
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "read",
            "--account",
            &beta,
            "--message",
            &message
        ])
        .await
        .unwrap()
        .status,
        4,
        "another account cannot read this local message ID"
    );
    // A rollback journal may not exist while the daemon uses WAL, but mail
    // downloads must never create one under the active profile.
    let protected = h.directory.join("store.db-journal");
    assert!(!protected.exists());
    for kind in ["raw", "attachment", "body"] {
        let mut arguments = vec![
            "--json",
            "mail",
            kind,
            "--account",
            &account,
            "--message",
            &message,
            "--output",
            protected.to_str().unwrap(),
        ];
        if kind == "attachment" {
            arguments.extend(["--attachment", attachment]);
        }
        if kind == "body" {
            arguments.extend(["--kind", "text"]);
        }
        let rejected = h.cli(&arguments).await.unwrap();
        assert_eq!(
            rejected.status, 1,
            "mail export must refuse a future protected journal"
        );
        assert!(
            !protected.exists(),
            "a rejected export must not create the journal"
        );
        assert_eq!(
            h.cli(&["--json", "system", "status"]).await.unwrap().status,
            0
        );
    }
    let raw = h.artifacts.join("download.eml");
    let pdf = h.artifacts.join("download.pdf");
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "raw",
            "--account",
            &account,
            "--message",
            &message,
            "--output",
            raw.to_str().unwrap()
        ])
        .await
        .unwrap()
        .status,
        0
    );
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "attachment",
            "--account",
            &account,
            "--message",
            &message,
            "--attachment",
            attachment,
            "--output",
            pdf.to_str().unwrap()
        ])
        .await
        .unwrap()
        .status,
        0
    );
    let expected = include_str!("../fixtures/mime/multipart.eml")
        .replace("ACCOUNT", "alpha@example.test")
        .replace('\n', "\r\n");
    use sha2::{Digest, Sha256};
    assert_eq!(
        Sha256::digest(std::fs::read(raw).unwrap()),
        Sha256::digest(expected.as_bytes())
    );
    assert_eq!(
        Sha256::digest(std::fs::read(pdf).unwrap()),
        Sha256::digest(b"%PDF-1.4\nSynthetic PDF fixture\n%%EOF\n")
    );
    let search = h
        .cli(&[
            "--json",
            "mail",
            "search",
            "--account",
            &account,
            "--query",
            "searchable",
        ])
        .await
        .unwrap();
    assert_eq!(search.status, 0);
    assert_eq!(
        search.json().unwrap()["result"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await.requests).unwrap(),
        serde_json::to_value(remote_requests).unwrap(),
        "cached CLI reads must not make provider requests"
    );
    h.shutdown().await.unwrap();
}
fn browser_for_offline_check() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(1))
        .build()
        .unwrap()
}

#[tokio::test]
async fn gmail_projection_and_cursor_survive_daemon_crashes_at_each_commit_boundary() {
    for cap in [1, 10] {
        for checkpoint in [
            "gmail-before-page-commit",
            "gmail-before-promotion",
            "gmail-after-promotion",
        ] {
            let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
            h.google.control().set_page_cap(cap).await.unwrap();
            let account = h.connect_google("alpha@example.test").await.unwrap();
            assert_eq!(
                h.cli(&["--json", "sync", "--account", &account, "--wait"])
                    .await
                    .unwrap()
                    .status,
                0
            );
            let before = h
                .cli(&["--json", "mail", "list", "--account", &account])
                .await
                .unwrap()
                .json()
                .unwrap();
            let original = before["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["provider_id"] == "m-001")
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            h.google
                .control()
                .delete_message("alpha@example.test", "m-002")
                .await
                .unwrap();
            h.google
                .control()
                .change_labels(
                    "alpha@example.test",
                    "m-001",
                    &["STARRED".into()],
                    &["UNREAD".into()],
                )
                .await
                .unwrap();
            h.google.control().add_message("alpha@example.test","new-one","new-thread",b"From: sender@example.test\r\nSubject: Crash fixture\r\n\r\noriginal crash bytes\r\n".to_vec(),["INBOX".into()].into_iter().collect()).await.unwrap();
            let barriers = h.artifacts.join("barriers");
            std::fs::create_dir_all(&barriers).unwrap();
            std::fs::write(barriers.join(format!("{checkpoint}.arm")), []).unwrap();
            let started = h
                .cli(&["--json", "sync", "--account", &account, "--full"])
                .await
                .unwrap();
            assert_eq!(started.status, 0);
            let started = started.json().unwrap();
            let run = started["result"]["id"].as_str().unwrap();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            while !barriers.join(format!("{checkpoint}.entered")).exists() {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "daemon never entered {checkpoint}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let snapshot = h
                .cli(&["--json", "mail", "list", "--account", &account])
                .await
                .unwrap()
                .json()
                .unwrap();
            if checkpoint != "gmail-after-promotion" {
                let from = before["result"]["revision"].as_u64().unwrap();
                let to = snapshot["result"]["revision"].as_u64().unwrap();
                assert!(to > from);
                let changes = h
                    .cli(&[
                        "--json",
                        "system",
                        "watch",
                        "--after",
                        &from.to_string(),
                        "--limit",
                        &(to - from).to_string(),
                    ])
                    .await
                    .unwrap();
                assert_eq!(changes.status, 0);
                let changes = String::from_utf8(changes.stdout).unwrap();
                let changes: Vec<serde_json::Value> = changes
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(changes.len() as u64, to - from);
                for (index, change) in changes.iter().enumerate() {
                    assert_eq!(change["result"]["revision"], from + index as u64 + 1);
                    assert_eq!(change["result"]["kind"], "sync_run");
                    assert_eq!(change["result"]["resource_id"], run);
                }
                // Global progress is observable, while every field in the mail
                // projection and its coverage must remain identical.
                let mut expected = before.clone();
                expected["result"]["revision"] = serde_json::json!(to);
                assert_eq!(
                    snapshot, expected,
                    "staged changes became visible before promotion"
                );
            }
            h.force_kill().await.unwrap();
            h.restart().await.unwrap();
            let status = h
                .cli(&[
                    "--json",
                    "system",
                    "sync-status",
                    "--account",
                    &account,
                    "--run",
                    run,
                ])
                .await
                .unwrap();
            assert_eq!(status.status, 0);
            let status = status.json().unwrap();
            assert_eq!(
                status["result"]["state"],
                if checkpoint == "gmail-after-promotion" {
                    "succeeded"
                } else {
                    "failed"
                }
            );
            assert_eq!(
                h.cli(&["--json", "sync", "--account", &account, "--wait"])
                    .await
                    .unwrap()
                    .status,
                0
            );
            let final_mail = h
                .cli(&["--json", "mail", "list", "--account", &account])
                .await
                .unwrap()
                .json()
                .unwrap();
            let items = final_mail["result"]["items"].as_array().unwrap();
            assert_eq!(items.len(), 3);
            assert!(!items.iter().any(|m| m["provider_id"] == "m-002"));
            assert!(items.iter().any(|m| m["provider_id"] == "new-one"));
            let preserved = items.iter().find(|m| m["provider_id"] == "m-001").unwrap();
            assert_eq!(preserved["id"], original);
            assert!(preserved["collections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["provider_id"] == "STARRED"));
            assert!(!preserved["collections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["provider_id"] == "UNREAD"));
            let remote = h.google.control().snapshot().await;
            assert_eq!(
                final_mail["result"]["coverage"]["cursor"],
                remote.mail["alpha@example.test"].history_id
            );
            assert_eq!(remote.requests.iter().find(|r|r.account.as_deref()==Some("alpha@example.test")&&r.path=="/gmail/v1/users/me/profile").unwrap().count,2,"restart should use the retained history rather than restart a successful projection blindly");
            assert!(h
                .google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .is_empty());
            h.shutdown().await.unwrap();
        }
    }
}

#[tokio::test]
async fn google_accounts_connect_refresh_disconnect_and_reconnect_through_actual_cli() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let config = h.artifacts.join("synthetic-client.json");
    std::fs::write(
        &config,
        br#"{"installed":{"client_id":"nuncio-test-client","client_secret":"synthetic-client-secret"}}"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut ids = Vec::new();
    for address in ["alpha@example.test", "beta@example.test"] {
        let started = h
            .cli(&[
                "--json",
                "account",
                "connect-google",
                "--client-config",
                config.to_str().unwrap(),
                "--login-hint",
                address,
                "--no-browser",
            ])
            .await
            .unwrap();
        assert_eq!(
            started.status, 0,
            "actual CLI must start mock-backed browser OAuth"
        );
        let auth = started.json().unwrap()["result"].clone();
        let url = auth["browser_url"].as_str().unwrap();
        assert!(url.starts_with(h.google.base_url()));
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap();
        let response = http.get(url).send().await.unwrap();
        assert_eq!(response.status(), 302);
        let callback = response.headers()["location"].to_str().unwrap();
        assert_eq!(http.get(callback).send().await.unwrap().status(), 200);
        let session = auth["session_id"].as_str().unwrap();
        let status = h
            .cli(&["--json", "account", "auth-status", "--session", session])
            .await
            .unwrap();
        assert_eq!(status.status, 0);
        assert_eq!(status.json().unwrap()["result"]["state"], "succeeded");
        ids.push(
            status.json().unwrap()["result"]["account_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_ne!(ids[0], ids[1]);
    let list = h
        .cli(&["--json", "account", "list"])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(list["result"]["accounts"].as_array().unwrap().len(), 2);
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    h.google.control().rotate_refresh_tokens(true).await;
    h.google
        .control()
        .advance(std::time::Duration::from_secs(3601))
        .await;
    for id in &ids {
        assert_eq!(
            h.cli(&["--json", "account", "check", "--account", id])
                .await
                .unwrap()
                .status,
            0
        );
    }
    // The new refresh credentials must survive a process death, not merely a cache hit.
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(
        h.cli(&["--json", "account", "check", "--account", &ids[1]])
            .await
            .unwrap()
            .status,
        0
    );
    h.google.control().revoke("alpha@example.test").await;
    assert_eq!(
        h.cli(&["--json", "account", "check", "--account", &ids[0]])
            .await
            .unwrap()
            .status,
        3
    );
    assert_eq!(
        h.cli(&["--json", "account", "check", "--account", &ids[1]])
            .await
            .unwrap()
            .status,
        0
    );
    assert_eq!(
        h.cli(&["--json", "account", "disconnect", "--account", &ids[1]])
            .await
            .unwrap()
            .status,
        0
    );
    let list = h
        .cli(&["--json", "account", "list"])
        .await
        .unwrap()
        .json()
        .unwrap();
    let accounts = list["result"]["accounts"].as_array().unwrap();
    assert!(accounts
        .iter()
        .any(|a| a["id"] == ids[0] && a["state"] == "needs_auth"));
    assert!(accounts
        .iter()
        .any(|a| a["id"] == ids[1] && a["state"] == "disconnected"));
    let bytes = std::fs::read(&h.secrets_file).unwrap();
    let secrets: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(secrets
        .as_object()
        .unwrap()
        .keys()
        .all(|key| !key.contains(&ids[1])));
    let reauth = h
        .cli(&[
            "--json",
            "account",
            "connect-google",
            "--client-config",
            config.to_str().unwrap(),
            "--account",
            &ids[1],
            "--login-hint",
            "beta@example.test",
            "--no-browser",
        ])
        .await
        .unwrap();
    assert_eq!(reauth.status, 0);
    let auth = reauth.json().unwrap()["result"].clone();
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let response = http
        .get(auth["browser_url"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 302);
    assert_eq!(
        http.get(response.headers()["location"].to_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let status = h
        .cli(&[
            "--json",
            "account",
            "auth-status",
            "--session",
            auth["session_id"].as_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(status.json().unwrap()["result"]["account_id"], ids[1]);
    assert_eq!(status.json().unwrap()["result"]["state"], "succeeded");
    h.shutdown().await.unwrap();
    for entry in std::fs::read_dir(&h.artifacts).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "log") {
            let content = std::fs::read(path).unwrap();
            assert!(!content
                .windows(b"synthetic-client-secret".len())
                .any(|w| w == b"synthetic-client-secret"));
            assert!(!content
                .windows(b"mock-refresh-".len())
                .any(|w| w == b"mock-refresh-"));
        }
    }
    for name in ["store.db", "store.db-wal"] {
        if let Ok(content) = std::fs::read(h.directory.join(name)) {
            for canary in [
                "synthetic-client-secret",
                "mock-refresh-",
                "beta@example.test",
            ] {
                assert!(!content
                    .windows(canary.len())
                    .any(|w| w == canary.as_bytes()));
            }
        }
    }
}

#[tokio::test]
async fn calendar_canonical_and_expanded_agenda_are_available_through_actual_cli() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    let event: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/calendar/all-day-2.json")).unwrap();
    h.google
        .control()
        .put_event("alpha@example.test", "primary", event.clone())
        .await
        .unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let rolling = h
        .cli(&[
            "--json",
            "calendar",
            "refresh",
            "--account",
            &account,
            "--wait",
        ])
        .await
        .unwrap();
    assert_eq!(rolling.status, 0);
    let march = h
        .cli(&[
            "--json",
            "calendar",
            "agenda",
            "--account",
            &account,
            "--from",
            "2026-03-01",
            "--to",
            "2026-03-20",
        ])
        .await
        .unwrap();
    assert_eq!(march.status, 0);
    let march = march.json().unwrap();
    let moved = march["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["provider_id"] == "dstseries_20260308T140000Z")
        .unwrap();
    assert_eq!(
        moved["start"]["date_time"]["rfc3339"],
        "2026-03-08T10:30:00-05:00"
    );
    let master_id = moved["recurring_event_id"].as_str().unwrap();
    let calendar_id = moved["calendar_id"].as_str().unwrap();
    let refresh = h
        .cli(&[
            "--json",
            "calendar",
            "refresh",
            "--account",
            &account,
            "--from",
            "2026-10-30",
            "--to",
            "2026-11-03",
            "--wait",
        ])
        .await
        .unwrap();
    assert_eq!(
        refresh.status, 0,
        "calendar refresh must sync canonical resources and provider-expanded agenda"
    );
    let calendars = h
        .cli(&["--json", "calendar", "list", "--account", &account])
        .await
        .unwrap();
    assert_eq!(calendars.status, 0);
    let calendars = calendars.json().unwrap();
    assert_eq!(calendars["result"]["items"].as_array().unwrap().len(), 2);
    let requests_before = h
        .google
        .control()
        .snapshot()
        .await
        .requests
        .iter()
        .map(|r| r.count)
        .sum::<u64>();
    h.google.stop().await.unwrap();
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let master = h
        .cli(&[
            "--json",
            "calendar",
            "get",
            "--account",
            &account,
            "--calendar",
            calendar_id,
            "--event",
            master_id,
        ])
        .await
        .unwrap();
    assert_eq!(master.status, 0);
    let master = master.json().unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(master["result"]["event"]["provider_json"].as_str().unwrap()).unwrap();
    assert_eq!(raw["id"], "dstseries");
    assert_eq!(raw["recurrence"][0], "RRULE:FREQ=WEEKLY;COUNT=3");
    for (from, to, count) in [
        ("2026-10-31", "2026-11-01", 1),
        ("2026-11-01", "2026-11-02", 1),
        ("2026-11-02", "2026-11-03", 0),
    ] {
        let agenda = h
            .cli(&[
                "--json",
                "calendar",
                "agenda",
                "--account",
                &account,
                "--from",
                from,
                "--to",
                to,
            ])
            .await
            .unwrap();
        assert_eq!(agenda.status, 0);
        let agenda = agenda.json().unwrap();
        let items = agenda["result"]["items"].as_array().unwrap();
        assert_eq!(
            items
                .iter()
                .filter(|e| e["provider_id"] == "alldate02")
                .count(),
            count,
            "date-only end is exclusive"
        );
        if let Some(item) = items.iter().find(|e| e["provider_id"] == "alldate02") {
            assert_eq!(item["start"]["date"], "2026-10-31");
            assert_eq!(item["end"]["date"], "2026-11-02");
            let get = h
                .cli(&[
                    "--json",
                    "calendar",
                    "get",
                    "--account",
                    &account,
                    "--calendar",
                    item["calendar_id"].as_str().unwrap(),
                    "--event",
                    item["id"].as_str().unwrap(),
                ])
                .await
                .unwrap();
            assert_eq!(get.status, 0);
            let get = get.json().unwrap();
            let raw: serde_json::Value =
                serde_json::from_str(get["result"]["event"]["provider_json"].as_str().unwrap())
                    .unwrap();
            assert_eq!(raw["extendedProperties"], event["extendedProperties"]);
        }
    }
    let outside = h
        .cli(&[
            "--json",
            "calendar",
            "agenda",
            "--account",
            &account,
            "--from",
            "2028-01-01",
            "--to",
            "2028-01-02",
        ])
        .await
        .unwrap();
    assert_eq!(outside.status, 0);
    let outside = outside.json().unwrap();
    assert!(outside["result"]["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["state"] == "out_of_window"));
    assert_eq!(
        h.google
            .control()
            .snapshot()
            .await
            .requests
            .iter()
            .map(|r| r.count)
            .sum::<u64>(),
        requests_before
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn real_cli_daemon_restart_health_json_and_missing_daemon_exit() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let output = h.cli(&["--json", "system", "status"]).await.unwrap();
    assert_eq!(output.status, 0);
    assert!(output.stderr.is_empty());
    let id = output.json().unwrap()["result"]["profile_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(output.json().unwrap()["schema_version"], 1);
    let invalid = h.cli(&["--json", "made-up-command"]).await.unwrap();
    assert_eq!(invalid.status, 2);
    assert_eq!(invalid.json().unwrap()["error"]["code"], "invalid_input");
    assert!(!invalid.stderr.is_empty());
    let before = h.google.control().snapshot().await;
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let restarted = h.cli(&["--json", "system", "status"]).await.unwrap();
    assert_eq!(restarted.status, 0);
    assert_eq!(restarted.json().unwrap()["result"]["profile_id"], id);
    assert_eq!(
        h.google.control().snapshot().await.mail["alpha@example.test"].history_id,
        before.mail["alpha@example.test"].history_id
    );
    h.shutdown().await.unwrap();
    let offline = h.cli(&["--json", "system", "status"]).await.unwrap();
    assert_eq!(offline.status, 4);
    assert_eq!(offline.json().unwrap()["error"]["code"], "unavailable");
}

#[tokio::test]
async fn cli_change_watch_replays_jsonl_and_rejects_a_future_revision() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let run = h
        .cli(&["--json", "sync", "--account", &account, "--wait"])
        .await
        .unwrap();
    assert_eq!(run.status, 0);
    let output = h
        .cli(&["--json", "system", "watch", "--after", "0", "--limit", "2"])
        .await
        .unwrap();
    assert_eq!(output.status, 0);
    let lines = String::from_utf8(output.stdout).unwrap();
    let values: Vec<serde_json::Value> = lines
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(values.len(), 2);
    assert_eq!(values[0]["schema_version"], 1);
    assert_eq!(values[0]["result"]["revision"], 1);
    assert_eq!(values[1]["result"]["revision"], 2);
    assert_eq!(values[0]["result"]["account_id"], account);
    let invalid = h
        .cli(&[
            "--json",
            "system",
            "watch",
            "--after",
            "18446744073709551615",
            "--limit",
            "1",
        ])
        .await
        .unwrap();
    assert_eq!(invalid.status, 5);
    assert_eq!(
        invalid.json().unwrap()["error"]["code"],
        "resnapshot_required"
    );
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let resumed = h
        .cli(&["--json", "system", "watch", "--after", "1", "--limit", "1"])
        .await
        .unwrap();
    assert_eq!(resumed.status, 0);
    assert_eq!(resumed.json().unwrap()["result"]["revision"], 2);
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn account_sequences_coalesce_and_queued_calendar_does_not_block_another_account() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let alpha = h.connect_google("alpha@example.test").await.unwrap();
    let beta = h.connect_google("beta@example.test").await.unwrap();
    let barriers = h.artifacts.join("barriers");
    std::fs::write(barriers.join("gmail-before-promotion.arm"), []).unwrap();
    let run = h
        .cli(&["--json", "sync", "--account", &alpha, "--full"])
        .await
        .unwrap();
    assert_eq!(run.status, 0);
    let run = run.json().unwrap();
    let run_id = run["result"]["id"].as_str().unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !barriers.join("gmail-before-promotion.entered").exists() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let repeated = h
        .cli(&["--json", "sync", "--account", &alpha, "--full"])
        .await
        .unwrap();
    assert_eq!(repeated.status, 0);
    assert_eq!(repeated.json().unwrap()["result"]["id"], run_id);
    let calendar = h
        .cli(&["--json", "calendar", "refresh", "--account", &alpha])
        .await
        .unwrap();
    assert_eq!(calendar.status, 0);
    let calendar = calendar.json().unwrap();
    let calendar_run = calendar["result"]["id"].as_str().unwrap();
    let other = h
        .cli(&["--json", "sync", "--account", &beta, "--wait"])
        .await
        .unwrap();
    assert_eq!(
        other.status, 0,
        "Queued work for alpha consumed beta's global permit"
    );
    let status = h
        .cli(&[
            "--json",
            "system",
            "sync-status",
            "--account",
            &alpha,
            "--run",
            calendar_run,
        ])
        .await
        .unwrap();
    assert_eq!(status.status, 0);
    assert_eq!(status.json().unwrap()["result"]["state"], "queued");
    for id in [calendar_run, run_id] {
        let cancelled = h
            .cli(&[
                "--json",
                "system",
                "cancel-sync",
                "--account",
                &alpha,
                "--run",
                id,
            ])
            .await
            .unwrap();
        assert_eq!(cancelled.status, 0);
        assert_eq!(cancelled.json().unwrap()["result"]["state"], "cancelled");
    }
    let remote = h.google.control().snapshot().await;
    assert_eq!(
        remote
            .requests
            .iter()
            .filter(|r| r.account.as_deref() == Some("alpha@example.test")
                && r.path.starts_with("/calendar/"))
            .map(|r| r.count)
            .sum::<u64>(),
        0
    );
    let local = h
        .cli(&["--json", "mail", "list", "--account", &beta])
        .await
        .unwrap();
    assert_eq!(local.status, 0);
    assert_eq!(
        local.json().unwrap()["result"]["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn daemon_polling_refreshes_mail_and_calendar_without_manual_sync() {
    let mut h = E2eHarness::start_with_polling(Seed::TwoAccounts, Some(100))
        .await
        .unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    for changed in [false, true] {
        if changed {
            h.google
                .control()
                .change_labels("alpha@example.test", "m-001", &["STARRED".into()], &[])
                .await
                .unwrap();
            h.google.control().put_event("alpha@example.test","primary",serde_json::json!({"id":"alldate01","status":"confirmed","summary":"Background calendar update","start":{"date":"2026-03-09"},"end":{"date":"2026-03-10"}})).await.unwrap();
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            let mail = h
                .cli(&["--json", "mail", "list", "--account", &account])
                .await
                .unwrap();
            assert_eq!(mail.status, 0);
            let mail = mail.json().unwrap();
            let agenda = h
                .cli(&[
                    "--json",
                    "calendar",
                    "agenda",
                    "--account",
                    &account,
                    "--from",
                    "2026-03-01",
                    "--to",
                    "2026-03-20",
                ])
                .await
                .unwrap();
            assert_eq!(agenda.status, 0);
            let agenda = agenda.json().unwrap();
            let messages = mail["result"]["items"].as_array().unwrap();
            let events = agenda["result"]["items"].as_array().unwrap();
            let covered = agenda["result"]["coverage"].as_array().unwrap();
            let ready = messages.len() == 3
                && events.len() == 4
                && covered.len() == 2
                && covered.iter().all(|c| c["state"] == "current")
                && (!changed
                    || messages.iter().any(|m| {
                        m["provider_id"] == "m-001"
                            && m["collections"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|c| c["provider_id"] == "STARRED")
                    }) && events.iter().any(|e| {
                        e["provider_id"] == "alldate01"
                            && e["summary"] == "Background calendar update"
                    }));
            if ready {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Background polling did not converge (changed={changed})"
            );
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    }
    let remote = h.google.control().snapshot().await;
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    assert!(remote.calendars["alpha@example.test"]["alpha@example.test"]
        .notifications
        .is_empty());
    h.shutdown().await.unwrap();
}
