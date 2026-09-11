use super::*;
async fn barrier(h: &E2eHarness, name: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !h
            .artifacts
            .join("barriers")
            .join(format!("{name}.entered"))
            .exists()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn show(h: &E2eHarness, account: &str, id: &str) -> Value {
    result(
        h.cli(&[
            "--json",
            "operation",
            "show",
            "--account",
            account,
            "--operation",
            id,
        ])
        .await
        .unwrap(),
    )
}
fn write_count(snapshot: &nuncio_test_support::google::Snapshot) -> u64 {
    snapshot
        .requests
        .iter()
        .filter(|r| {
            r.method != "GET" && (r.path.starts_with("/gmail/") || r.path.starts_with("/calendar/"))
        })
        .map(|r| r.count)
        .sum()
}
#[tokio::test]
async fn actual_cli_safe_resume_survives_sigkill_after_remote_mail_and_calendar_writes_without_duplicates(
) {
    for kind in ["mail", "calendar_none", "calendar_all"] {
        let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
        let account = h.connect_google("alpha@example.test").await.unwrap();
        let file = h.artifacts.join("change.json");
        let request_id = "f3e0f7e0-1a2a-4614-8c26-c0e850cc913b";
        let original = h.directory.clone();
        let barriers = h.artifacts.join("barriers");
        let queued = if kind == "mail" {
            result(
                h.cli(&["--json", "sync", "--account", &account, "--wait"])
                    .await
                    .unwrap(),
            );
            let mail = result(
                h.cli(&["--json", "mail", "list", "--account", &account])
                    .await
                    .unwrap(),
            );
            let id = mail["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["provider_id"] == "m-001")
                .unwrap()["id"]
                .as_str()
                .unwrap();
            std::fs::write(&file, br#"{"schema_version":1,"action":"archive"}"#).unwrap();
            std::fs::write(barriers.join("operation_before_dispatch.arm"), []).unwrap();
            result(
                h.cli(&[
                    "--json",
                    "mail",
                    "change",
                    "--account",
                    &account,
                    "--message",
                    id,
                    "--request-id",
                    request_id,
                    "--file",
                    file.to_str().unwrap(),
                ])
                .await
                .unwrap(),
            )
        } else {
            h.google.control().put_event("alpha@example.test","alpha@example.test",json!({"id":"recover001","summary":"Original","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"},"attendees":[{"email":"recipient@example.test"}],"provider_extension":{"retain":true}})).await.unwrap();
            result(
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
                    "--wait",
                ])
                .await
                .unwrap(),
            );
            let agenda = result(
                h.cli(&[
                    "--json",
                    "calendar",
                    "agenda",
                    "--account",
                    &account,
                    "--from",
                    "2026-10-01",
                    "--to",
                    "2026-10-05",
                ])
                .await
                .unwrap(),
            );
            let event = agenda["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["provider_id"] == "recover001")
                .unwrap();
            let change = json!({"schema_version":1,"action":"update","scope":"single","notifications":if kind=="calendar_all" {"all"} else {"none"},"event_id":event["id"],"expected_etag":event["etag"],"patch":{"summary":"Recovered desired title"}});
            std::fs::write(&file, serde_json::to_vec(&change).unwrap()).unwrap();
            std::fs::write(barriers.join("operation_before_dispatch.arm"), []).unwrap();
            result(
                h.cli(&[
                    "--json",
                    "calendar",
                    "change",
                    "--account",
                    &account,
                    "--calendar",
                    event["calendar_id"].as_str().unwrap(),
                    "--request-id",
                    request_id,
                    "--file",
                    file.to_str().unwrap(),
                ])
                .await
                .unwrap(),
            )
        };
        barrier(&h, "operation_before_dispatch").await;
        let id = queued["id"].as_str().unwrap();
        let backup = h.artifacts.join("queued.nuncio");
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
            .await
            .unwrap(),
        );
        let cipher = std::fs::read(&backup).unwrap();
        let restored = result(
            h.cli_with_stdin(
                &[
                    "--json",
                    "backup",
                    "restore",
                    "--file",
                    backup.to_str().unwrap(),
                    "--new-profile",
                    "recovered",
                ],
                SECRET,
            )
            .await
            .unwrap(),
        );
        assert_eq!(restored["held_operations"], 1);
        let before = h.google.control().snapshot().await;
        assert_eq!(write_count(&before), 0);
        h.force_kill().await.unwrap();
        h.directory = restored["directory"].as_str().unwrap().into();
        h.restart().await.unwrap();
        assert_eq!(
            h.connect_google("alpha@example.test").await.unwrap(),
            account
        );
        let held = show(&h, &account, id).await;
        let observed = h
            .cli(&[
                "--json",
                "operation",
                "reconcile",
                "--account",
                &account,
                "--operation",
                id,
                "--request-id",
                "78c7ab5c-48a6-45ee-8f40-0e0df46d9bca",
                "--version",
                &held["version"].to_string(),
                "--wait",
            ])
            .await
            .unwrap();
        assert_eq!(observed.status, 5);
        let observed = show(&h, &account, id).await;
        assert_eq!(observed["error_code"], "reconciliation_resume_required");
        assert_eq!(write_count(&h.google.control().snapshot().await), 0);
        let version = observed["version"].to_string();
        let resume = [
            "--json",
            "operation",
            "reconcile",
            "--account",
            &account,
            "--operation",
            id,
            "--request-id",
            "ef7cce7e-8e3a-4b51-8f88-b4d4b12454cf",
            "--version",
            &version,
            "--resume-safe",
        ];
        // Hold the finished observation until the dispatch checkpoint is armed.
        std::fs::write(barriers.join("operation_job_finished.arm"), []).unwrap();
        result(h.cli(&resume).await.unwrap());
        barrier(&h, "operation_job_finished").await;
        assert_eq!(show(&h, &account, id).await["state"], "retry_wait");
        assert_eq!(write_count(&h.google.control().snapshot().await), 0);
        std::fs::write(barriers.join("operation_before_receipt.arm"), []).unwrap();
        std::fs::write(barriers.join("operation_job_finished.release"), []).unwrap();
        barrier(&h, "operation_before_receipt").await;
        let accepted = h.google.control().snapshot().await;
        assert_eq!(write_count(&accepted), 1);
        h.force_kill().await.unwrap();
        h.restart().await.unwrap();
        let wait = h
            .cli(&[
                "--json",
                "operation",
                "wait",
                "--account",
                &account,
                "--operation",
                id,
            ])
            .await
            .unwrap();
        assert_eq!(wait.status, if kind == "calendar_all" { 5 } else { 0 });
        let final_op = show(&h, &account, id).await;
        assert_eq!(final_op["request_id"], queued["request_id"]);
        assert_eq!(final_op["desired_state_json"], queued["desired_state_json"]);
        if kind == "calendar_all" {
            assert_eq!(final_op["error_code"], "calendar_notifications_unconfirmed");
        }
        assert_eq!(result(h.cli(&resume).await.unwrap()), final_op);
        let remote = h.google.control().snapshot().await;
        assert_eq!(write_count(&remote), 1);
        assert_eq!(
            serde_json::to_value(&remote.mail).unwrap(),
            serde_json::to_value(&accepted.mail).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&remote.calendars).unwrap(),
            serde_json::to_value(&accepted.calendars).unwrap()
        );
        assert!(remote.mail["alpha@example.test"].accepted_sends.is_empty());
        if kind == "mail" {
            assert!(!remote.mail["alpha@example.test"].messages["m-001"]
                .labels
                .contains("INBOX"));
        } else {
            let calendar = &remote.calendars["alpha@example.test"]["alpha@example.test"];
            assert_eq!(
                calendar.events["recover001"]["summary"],
                "Recovered desired title"
            );
            assert_eq!(
                calendar.events["recover001"]["provider_extension"],
                json!({"retain":true})
            );
            assert_eq!(
                calendar.notifications.len(),
                usize::from(kind == "calendar_all")
            );
        }
        let history = result(
            h.cli(&[
                "--json",
                "operation",
                "attempts",
                "--account",
                &account,
                "--operation",
                id,
            ])
            .await
            .unwrap(),
        );
        let attempts = history["items"].as_array().unwrap();
        assert_eq!(
            attempts.iter().filter(|a| a["kind"] == "dispatch").count(),
            1
        );
        assert_eq!(attempts.len(), 4);
        assert_eq!(
            attempts
                .iter()
                .map(|a| a["ordinal"].as_u64().unwrap())
                .collect::<std::collections::BTreeSet<_>>(),
            (1..=4).collect()
        );
        assert_eq!(final_op["reconciliation"]["first_attempt_ordinal"], 2);
        let interrupted = attempts.iter().find(|a| a["kind"] == "dispatch").unwrap();
        assert!(interrupted["receipts"].as_array().unwrap().is_empty());
        if kind == "calendar_all" {
            let decision = h.artifacts.join("abandon.json");
            std::fs::write(&decision,br#"{"decision":"abandon","reason":"Keep the event; notification outcome remains unknown"}"#).unwrap();
            let abandoned = result(
                h.cli(&[
                    "--json",
                    "operation",
                    "resolve",
                    "--account",
                    &account,
                    "--operation",
                    id,
                    "--version",
                    &final_op["version"].to_string(),
                    "--file",
                    decision.to_str().unwrap(),
                ])
                .await
                .unwrap(),
            );
            assert_eq!(abandoned["state"], "uncertain");
            assert_eq!(abandoned["disposition"], "abandoned");
            assert_eq!(
                abandoned["error_code"],
                "calendar_notifications_unconfirmed"
            );
            assert_eq!(
                result(
                    h.cli(&[
                        "--json",
                        "operation",
                        "attempts",
                        "--account",
                        &account,
                        "--operation",
                        id
                    ])
                    .await
                    .unwrap()
                )["items"],
                history["items"]
            );
            h.force_kill().await.unwrap();
            h.restart().await.unwrap();
            assert_eq!(show(&h, &account, id).await, abandoned);
            let after = h.google.control().snapshot().await;
            assert_eq!(write_count(&after), 1);
            assert_eq!(
                serde_json::to_value(&after.calendars).unwrap(),
                serde_json::to_value(&remote.calendars).unwrap()
            );
        }
        assert_eq!(std::fs::read(&backup).unwrap(), cipher);
        assert!(original.join("store.db").is_file());
        h.shutdown().await.unwrap();
    }
}
