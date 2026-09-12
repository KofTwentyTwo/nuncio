use super::{E2eHarness, Seed};
use serde_json::{json, Value};

async fn change(
    h: &E2eHarness,
    account: &str,
    calendar: &str,
    id: &str,
    mut action: Value,
) -> Value {
    action["schema_version"] = json!(1);
    let path = h.artifacts.join(format!("calendar-{id}.json"));
    std::fs::write(&path, serde_json::to_vec(&action).unwrap()).unwrap();
    let result = h
        .cli(&[
            "--json",
            "calendar",
            "change",
            "--account",
            account,
            "--calendar",
            calendar,
            "--request-id",
            id,
            "--file",
            path.to_str().unwrap(),
            "--wait",
        ])
        .await
        .unwrap();
    assert_eq!(
        result.status,
        0,
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    result.json().unwrap()["result"].clone()
}
async fn get(h: &E2eHarness, account: &str, calendar: &str, event: &str) -> Value {
    let result = h
        .cli(&[
            "--json",
            "calendar",
            "get",
            "--account",
            account,
            "--calendar",
            calendar,
            "--event",
            event,
        ])
        .await
        .unwrap();
    assert_eq!(result.status, 0);
    result.json().unwrap()["result"]["event"].clone()
}
#[tokio::test]
async fn calendar_cli_limited_writer_has_real_effects_without_private_access() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let provider_calendar = "team-alpha@example.test";
    h.google
        .control()
        .set_calendar_role(
            "alpha@example.test",
            provider_calendar,
            "writerWithoutPrivateAccess",
        )
        .await
        .unwrap();
    h.google.control().put_event("alpha@example.test", provider_calendar, json!({"id":"private001","visibility":"private","summary":"Private CLI canary","description":"Private body canary","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"}})).await.unwrap();
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
    let catalog = h
        .cli(&["--json", "calendar", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let calendar = catalog["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["provider_id"] == provider_calendar)
        .unwrap();
    assert_eq!(calendar["access_role"], "writerWithoutPrivateAccess");
    let calendar = calendar["id"].as_str().unwrap();
    let agenda = h
        .cli(&[
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
        .unwrap()
        .json()
        .unwrap();
    let private = agenda["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["provider_id"] == "private001")
        .unwrap();
    assert!(private["summary"].is_null());
    assert!(!private.to_string().contains("canary"));
    let private_id = private["id"].as_str().unwrap();
    let before = h.google.control().snapshot().await.calendars["alpha@example.test"]
        [provider_calendar]
        .clone();
    let create = json!({"action":"create","scope":"single","notifications":"all","event":{"summary":"Limited writer CLI","start":{"date":"2026-10-03"},"end":{"date":"2026-10-04"},"attendees":[{"email":"alpha@example.test"}]}});
    let created = change(
        &h,
        &account,
        calendar,
        "9182ea19-966d-4095-8396-5025868bd014",
        create,
    )
    .await;
    assert_eq!(created["state"], "applied");
    let id = created["resource_id"].as_str().unwrap();
    let local = get(&h, &account, calendar, id).await;
    let provider_id = local["provider_id"].as_str().unwrap();
    let edited = change(&h,&account,calendar,"dd06ee37-d5a4-4b6d-9ed1-7ccbc7e0a328",json!({"action":"update","event_id":id,"expected_etag":local["etag"],"scope":"single","notifications":"all","patch":{"summary":"Limited writer edited"}})).await;
    assert_eq!(edited["state"], "applied");
    let local = get(&h, &account, calendar, id).await;
    let responded = change(&h,&account,calendar,"8e4384f5-8bb7-4784-bf22-49c3efb2db9b",json!({"action":"respond","event_id":id,"expected_etag":local["etag"],"scope":"single","notifications":"none","response":"accepted"})).await;
    assert_eq!(responded["state"], "applied");
    let remote = h.google.control().snapshot().await.calendars["alpha@example.test"]
        [provider_calendar]
        .clone();
    assert_eq!(
        remote.events[provider_id]["summary"],
        "Limited writer edited"
    );
    assert_eq!(
        remote.events[provider_id]["attendees"][0]["responseStatus"],
        "accepted"
    );
    let local = get(&h, &account, calendar, id).await;
    let deleted = change(&h,&account,calendar,"1a13cbb3-389b-448d-b865-c1cf708122b7",json!({"action":"delete","event_id":id,"expected_etag":local["etag"],"scope":"single","notifications":"all"})).await;
    assert_eq!(deleted["state"], "applied");
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let private = get(&h, &account, calendar, private_id).await;
    assert!(private["summary"].is_null());
    assert!(!private.to_string().contains("canary"));
    let path = h.artifacts.join("private-denied.json");
    for action in ["update", "delete", "respond"] {
        let mut payload = json!({"schema_version":1,"action":action,"event_id":private_id,"expected_etag":private["etag"],"scope":"single","notifications":"all"});
        if action == "update" {
            payload["patch"] = json!({"summary":"Forbidden"});
        }
        if action == "respond" {
            payload["response"] = json!("accepted");
        }
        std::fs::write(&path, payload.to_string()).unwrap();
        assert_eq!(
            h.cli(&[
                "--json",
                "calendar",
                "change",
                "--account",
                &account,
                "--calendar",
                calendar,
                "--request-id",
                "d2512fb3-82cd-439b-9909-a04e481267e3",
                "--file",
                path.to_str().unwrap(),
                "--wait"
            ])
            .await
            .unwrap()
            .status,
            2
        );
    }
    let remote = h.google.control().snapshot().await;
    let final_calendar = &remote.calendars["alpha@example.test"][provider_calendar];
    assert_eq!(
        final_calendar.events["private001"],
        before.events["private001"]
    );
    assert_eq!(final_calendar.events[provider_id]["status"], "cancelled");
    assert_eq!(
        final_calendar.notifications.len(),
        before.notifications.len() + 3
    );
    assert!(final_calendar
        .notifications
        .iter()
        .all(|n| n.event_id == provider_id && n.recipients == vec!["alpha@example.test"]));
    assert_eq!(
        remote
            .requests
            .iter()
            .filter(|r| r.path.ends_with("/private001")
                && ["PATCH", "DELETE"].contains(&r.method.as_str()))
            .map(|r| r.count)
            .sum::<u64>(),
        0
    );
    std::fs::write(
        h.artifacts.join("limited-writer-effects.json"),
        serde_json::to_vec_pretty(&final_calendar).unwrap(),
    )
    .unwrap();
    h.shutdown().await.unwrap();
}
#[tokio::test]
async fn calendar_cli_writes_create_edit_respond_and_delete_with_independent_effects() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    assert_eq!(
        h.cli(&[
            "--json",
            "calendar",
            "refresh",
            "--account",
            &account,
            "--wait"
        ])
        .await
        .unwrap()
        .status,
        0
    );
    let catalog = h
        .cli(&["--json", "calendar", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let calendar = catalog["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["is_primary"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let before = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    let create = json!({"action":"create","scope":"single","notifications":"all","event":{"summary":"CLI Calendar","description":"Initial","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"},"attendees":[{"email":"alpha@example.test"},{"email":"outside@example.test"}]}});
    let created = change(
        &h,
        &account,
        calendar,
        "f1077565-7d11-4d8c-b4c2-1a229a398392",
        create.clone(),
    )
    .await;
    assert_eq!(created["state"], "applied");
    let duplicate = change(
        &h,
        &account,
        calendar,
        "f1077565-7d11-4d8c-b4c2-1a229a398392",
        create,
    )
    .await;
    assert_eq!(duplicate["id"], created["id"]);
    let local_id = created["resource_id"].as_str().unwrap();
    let local = get(&h, &account, calendar, local_id).await;
    let provider = local["provider_id"].as_str().unwrap().to_string();
    let after = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    assert_eq!(after.events.len(), before.events.len() + 1);
    assert_eq!(after.notifications.len(), before.notifications.len() + 1);
    assert_eq!(
        after.notifications.last().unwrap().recipients,
        vec!["alpha@example.test", "outside@example.test"]
    );
    change(&h,&account,calendar,"e94f5ddd-5ba2-41d9-a893-a7cb8584a01b",json!({"action":"update","event_id":local_id,"expected_etag":local["etag"],"scope":"single","notifications":"external_only","patch":{"summary":"Timed CLI event","start":{"date_time":{"rfc3339":"2026-10-02T10:00:00-05:00","time_zone":"America/Chicago"}},"end":{"date_time":{"rfc3339":"2026-10-02T11:00:00-05:00","time_zone":"America/Chicago"}}},"clear_fields":["description"]})).await;
    let timed = get(&h, &account, calendar, local_id).await;
    let after = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    assert_eq!(after.events[&provider]["summary"], "Timed CLI event");
    assert!(after.events[&provider].get("description").is_none());
    assert!(after.events[&provider]["start"].get("date").is_none());
    assert_eq!(after.notifications.len(), before.notifications.len() + 2);
    assert_eq!(
        after.notifications.last().unwrap().recipients,
        vec!["outside@example.test"]
    );
    change(&h,&account,calendar,"a562e66b-5041-4f54-aaca-3bdb74f41692",json!({"action":"respond","event_id":local_id,"expected_etag":timed["etag"],"scope":"single","notifications":"none","response":"tentative","comment":"Maybe"})).await;
    let responded = get(&h, &account, calendar, local_id).await;
    let after_response = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    assert_eq!(
        after_response.events[&provider]["attendees"][0]["responseStatus"],
        "tentative"
    );
    assert_eq!(
        after_response.events[&provider]["attendees"][1],
        after.events[&provider]["attendees"][1]
    );
    assert_eq!(
        after_response.notifications.len(),
        after.notifications.len()
    );
    change(&h,&account,calendar,"c5ebc90e-efb5-4200-a535-5c185f6f64af",json!({"action":"delete","event_id":local_id,"expected_etag":responded["etag"],"scope":"single","notifications":"all"})).await;
    assert_eq!(
        get(&h, &account, calendar, local_id).await["status"],
        "cancelled"
    );
    let final_state = h.google.control().snapshot().await;
    let remote = &final_state.calendars["alpha@example.test"]["alpha@example.test"];
    assert_eq!(remote.events[&provider]["status"], "cancelled");
    assert_eq!(remote.notifications.len(), before.notifications.len() + 3);
    assert!(final_state.mail["alpha@example.test"]
        .accepted_sends
        .is_empty());
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_cli_crash_boundaries_recover_without_duplicate_events_or_notifications() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for kind in ["create", "update", "delete"] {
        for boundary in [
            "operation_after_attempt",
            "operation_before_receipt",
            "remote_ack",
        ] {
            for notifications in ["none", "all"] {
                let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
                h.google.control().put_event("alpha@example.test","primary",json!({"id":"crashcal01","summary":"Original","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"},"attendees":[{"email":"recipient@example.test"}],"provider_extension":"retained"})).await.unwrap();
                let account = h.connect_google("alpha@example.test").await.unwrap();
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
                let agenda = h
                    .cli(&[
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
                    .unwrap()
                    .json()
                    .unwrap();
                let event = agenda["result"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["provider_id"] == "crashcal01")
                    .unwrap();
                let calendar = event["calendar_id"].as_str().unwrap();
                let mut action = match kind {
                    "create" => {
                        json!({"action":"create","event":{"summary":"Created exactly once","start":{"date":"2026-10-03"},"end":{"date":"2026-10-04"},"attendees":[{"email":"recipient@example.test"}]}})
                    }
                    "update" => {
                        json!({"action":"update","event_id":event["id"],"expected_etag":event["etag"],"patch":{"summary":"Changed exactly once"}})
                    }
                    _ => {
                        json!({"action":"delete","event_id":event["id"],"expected_etag":event["etag"]})
                    }
                };
                action["schema_version"] = json!(1);
                action["scope"] = json!("single");
                action["notifications"] = json!(notifications);
                let (method, path) = match kind {
                    "create" => ("POST", "/calendar/v3/calendars/alpha@example.test/events"),
                    "update" => (
                        "PATCH",
                        "/calendar/v3/calendars/alpha@example.test/events/crashcal01",
                    ),
                    _ => (
                        "DELETE",
                        "/calendar/v3/calendars/alpha@example.test/events/crashcal01",
                    ),
                };
                if boundary == "remote_ack" {
                    h.google
                        .control()
                        .inject(Fault {
                            method: method.into(),
                            path: path.into(),
                            account: Some("alpha@example.test".into()),
                            call: None,
                            phase: Phase::After,
                            action: FaultAction::Withhold {
                                barrier: "calendar-accepted".into(),
                            },
                        })
                        .await;
                } else {
                    std::fs::write(
                        h.artifacts.join("barriers").join(format!("{boundary}.arm")),
                        [],
                    )
                    .unwrap();
                }
                let before = h.google.control().snapshot().await.calendars["alpha@example.test"]
                    ["alpha@example.test"]
                    .clone();
                let file = h.artifacts.join("calendar-crash.json");
                std::fs::write(&file, serde_json::to_vec(&action).unwrap()).unwrap();
                let args = [
                    "--json",
                    "calendar",
                    "change",
                    "--account",
                    &account,
                    "--calendar",
                    calendar,
                    "--request-id",
                    "1a659692-6bba-484d-bf99-c79464676bb3",
                    "--file",
                    file.to_str().unwrap(),
                ];
                let queued = h.cli(&args).await.unwrap();
                assert_eq!(queued.status, 0);
                let queued = queued.json().unwrap();
                let id = queued["result"]["id"].as_str().unwrap();
                if boundary == "remote_ack" {
                    h.google
                        .control()
                        .wait_for_barrier("calendar-accepted")
                        .await
                        .unwrap();
                } else {
                    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
                    while !h
                        .artifacts
                        .join("barriers")
                        .join(format!("{boundary}.entered"))
                        .exists()
                    {
                        assert!(tokio::time::Instant::now() < deadline);
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }
                let remote = h.google.control().snapshot().await.calendars["alpha@example.test"]
                    ["alpha@example.test"]
                    .clone();
                assert_eq!(
                    remote.version,
                    before.version + u64::from(boundary != "operation_after_attempt")
                );
                h.force_kill().await.unwrap();
                if boundary == "remote_ack" {
                    h.google
                        .control()
                        .release_barrier("calendar-accepted")
                        .await;
                }
                h.restart().await.unwrap();
                let mut waited = args.to_vec();
                waited.push("--wait");
                let result = h.cli(&waited).await.unwrap();
                let unknown = notifications == "all" && boundary != "operation_after_attempt";
                assert_eq!(
                    result.status,
                    if unknown { 5 } else { 0 },
                    "{kind}/{boundary}/{notifications}: {}",
                    String::from_utf8_lossy(&result.stdout)
                );
                let shown = h
                    .cli(&[
                        "--json",
                        "operation",
                        "show",
                        "--account",
                        &account,
                        "--operation",
                        id,
                    ])
                    .await
                    .unwrap();
                assert_eq!(shown.status, 0);
                let shown = shown.json().unwrap();
                let op = &shown["result"];
                assert_eq!(op["state"], if unknown { "uncertain" } else { "applied" });
                assert_eq!(op["needs_reconciliation"], false);
                if unknown {
                    assert_eq!(op["error_code"], "calendar_notifications_unconfirmed");
                }
                let local = get(&h, &account, calendar, op["resource_id"].as_str().unwrap()).await;
                let snapshot = h.google.control().snapshot().await;
                let final_remote = &snapshot.calendars["alpha@example.test"]["alpha@example.test"];
                assert_eq!(final_remote.version, before.version + 1);
                assert_eq!(
                    final_remote.notifications.len(),
                    before.notifications.len() + usize::from(notifications == "all")
                );
                assert_eq!(
                    snapshot
                        .requests
                        .iter()
                        .filter(|r| r.method == method && r.path == path)
                        .map(|r| r.count)
                        .sum::<u64>(),
                    1
                );
                let provider = local["provider_id"].as_str().unwrap();
                let observed =
                    serde_json::from_str::<Value>(local["provider_json"].as_str().unwrap())
                        .unwrap();
                if kind == "delete" && boundary == "operation_after_attempt" {
                    // DELETE acknowledges with no event body. Retain the known
                    // snapshot as a tombstone without inventing a remote ETag.
                    let mut expected = before.events[provider].clone();
                    expected.as_object_mut().unwrap().remove("etag");
                    expected["status"] = json!("cancelled");
                    assert_eq!(observed, expected);
                    assert!(local["etag"].is_null());
                    assert_eq!(final_remote.events[provider]["status"], "cancelled");
                    assert_ne!(
                        final_remote.events[provider]["etag"],
                        before.events[provider]["etag"]
                    );
                } else {
                    assert_eq!(observed, final_remote.events[provider]);
                }
                assert_eq!(
                    final_remote.events.len(),
                    before.events.len() + usize::from(kind == "create")
                );
                if kind != "create" {
                    assert_eq!(
                        final_remote.events[provider]["provider_extension"],
                        "retained"
                    );
                }
                assert!(snapshot.mail["alpha@example.test"]
                    .accepted_sends
                    .is_empty());
                h.shutdown().await.unwrap();
            }
        }
    }
}

async fn refresh_october(h: &E2eHarness, account: &str) -> Value {
    assert_eq!(
        h.cli(&[
            "--json",
            "calendar",
            "refresh",
            "--account",
            account,
            "--from",
            "2026-10-24",
            "--to",
            "2026-11-10",
            "--wait"
        ])
        .await
        .unwrap()
        .status,
        0
    );
    let result = h
        .cli(&[
            "--json",
            "calendar",
            "agenda",
            "--account",
            account,
            "--from",
            "2026-10-24",
            "--to",
            "2026-11-10",
        ])
        .await
        .unwrap();
    assert_eq!(result.status, 0);
    result.json().unwrap()["result"].clone()
}
#[tokio::test]
async fn calendar_cli_series_scope_and_moved_instance_preserve_dst_and_identity() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    refresh_october(&h, &account).await;
    let catalog = h
        .cli(&["--json", "calendar", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let calendar = catalog["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["is_primary"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let before = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    let created=change(&h,&account,calendar,"a5ca49c9-4c0d-46b1-af20-69959f7ef9f2",json!({"action":"create","scope":"series","notifications":"all","event":{"summary":"Recurring CLI","start":{"date_time":{"rfc3339":"2026-10-25T09:00:00-05:00","time_zone":"America/Chicago"}},"end":{"date_time":{"rfc3339":"2026-10-25T10:00:00-05:00","time_zone":"America/Chicago"}},"recurrence":["RRULE:FREQ=WEEKLY;COUNT=3"],"attendees":[{"email":"guest@example.test"}]}})).await;
    let master_id = created["resource_id"].as_str().unwrap();
    let master = get(&h, &account, calendar, master_id).await;
    let provider = master["provider_id"].as_str().unwrap();
    change(&h,&account,calendar,"0e64d4ef-1bc6-4f9c-8d7d-5909a1a51f16",json!({"action":"update","event_id":master_id,"expected_etag":master["etag"],"scope":"series","notifications":"all","patch":{"summary":"All occurrences"}})).await;
    let agenda = refresh_october(&h, &account).await;
    let occurrences: Vec<_> = agenda["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["recurring_provider_id"] == provider)
        .collect();
    assert_eq!(occurrences.len(), 3);
    assert!(occurrences
        .iter()
        .all(|e| e["summary"] == "All occurrences"));
    assert_eq!(
        occurrences[0]["start"]["date_time"]["rfc3339"],
        "2026-10-25T09:00:00-05:00"
    );
    assert_eq!(
        occurrences[1]["start"]["date_time"]["rfc3339"],
        "2026-11-01T09:00:00-06:00"
    );
    let instance = occurrences[1];
    let moved=change(&h,&account,calendar,"23d0a4a5-e6d0-42f4-bf6e-dbb51a67fa5b",json!({"action":"update","event_id":instance["id"],"expected_etag":instance["etag"],"scope":"single","notifications":"all","patch":{"start":{"date_time":{"rfc3339":"2026-11-02T10:30:00-06:00","time_zone":"America/Chicago"}},"end":{"date_time":{"rfc3339":"2026-11-02T11:30:00-06:00","time_zone":"America/Chicago"}}}})).await;
    assert_eq!(moved["resource_id"], instance["id"]);
    let current = get(&h, &account, calendar, instance["id"].as_str().unwrap()).await;
    assert_eq!(current["original_start"], instance["original_start"]);
    assert_eq!(current["provider_id"], instance["provider_id"]);
    let after_refresh = refresh_october(&h, &account).await;
    let updated: Vec<_> = after_refresh["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["recurring_provider_id"] == provider)
        .collect();
    assert_eq!(updated.len(), 3);
    for original in &occurrences {
        let current = updated.iter().find(|e| e["id"] == original["id"]).unwrap();
        assert_eq!(current["provider_id"], original["provider_id"]);
        if original["id"] == instance["id"] {
            assert_eq!(
                current["start"]["date_time"]["rfc3339"],
                "2026-11-02T10:30:00-06:00"
            );
        } else {
            assert_eq!(current["start"], original["start"]);
        }
    }
    let after = h.google.control().snapshot().await;
    let remote = &after.calendars["alpha@example.test"]["alpha@example.test"];
    assert_eq!(remote.events.len(), before.events.len() + 2);
    assert_eq!(remote.notifications.len(), before.notifications.len() + 3);
    assert_eq!(remote.events[provider]["summary"], "All occurrences");
    assert_eq!(
        remote.events[instance["provider_id"].as_str().unwrap()]["originalStartTime"]["dateTime"],
        "2026-11-01T09:00:00-06:00"
    );
    let current_master = get(&h, &account, calendar, master_id).await;
    for (scope, target, etag) in [
        ("single", master_id, current_master["etag"].clone()),
        (
            "series",
            instance["id"].as_str().unwrap(),
            current["etag"].clone(),
        ),
        ("following", master_id, current_master["etag"].clone()),
    ] {
        let path = h.artifacts.join(format!("invalid-{scope}.json"));
        std::fs::write(&path,serde_json::to_vec(&json!({"schema_version":1,"action":"update","event_id":target,"expected_etag":etag,"scope":scope,"notifications":"all","patch":{"summary":"Must not apply"}})).unwrap()).unwrap();
        assert_eq!(
            h.cli(&[
                "--json",
                "calendar",
                "change",
                "--account",
                &account,
                "--calendar",
                calendar,
                "--request-id",
                "84b5e6d2-dd2d-4dbb-83dd-6a117b313914",
                "--file",
                path.to_str().unwrap(),
                "--wait"
            ])
            .await
            .unwrap()
            .status,
            2
        );
    }
    let final_remote = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    assert_eq!(final_remote.events, remote.events);
    assert_eq!(final_remote.version, remote.version);
    assert_eq!(final_remote.notifications.len(), remote.notifications.len());
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_freebusy_cli_reports_partial_coverage_and_validates_action_file() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    h.google.control().put_event("alpha@example.test","alpha@example.test",json!({"id":"availability001","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"}})).await.unwrap();
    let path = h.artifacts.join("freebusy.json");
    let q = json!({"schema_version":1,"from":"2026-10-02T00:00:00Z","to":"2026-10-04T00:00:00Z","time_zone":"America/Chicago","provider_calendar_ids":["primary","team-alpha@example.test","missing@example.test"]});
    std::fs::write(&path, serde_json::to_vec(&q).unwrap()).unwrap();
    let before = h.google.control().snapshot().await;
    let output = h
        .cli(&[
            "--json",
            "calendar",
            "free-busy",
            "--account",
            &account,
            "--file",
            path.to_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(
        output.status,
        0,
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let r = output.json().unwrap()["result"].clone();
    assert_eq!(r["complete"], false);
    assert_eq!(r["from"], q["from"]);
    assert_eq!(r["to"], q["to"]);
    assert_eq!(
        r["calendars"][0]["busy"],
        json!([{"from":"2026-10-02T00:00:00-05:00","to":"2026-10-03T00:00:00-05:00"}])
    );
    assert_eq!(r["calendars"][1]["complete"], true);
    assert_eq!(r["calendars"][1]["busy"], json!([]));
    assert_eq!(r["calendars"][2]["complete"], false);
    assert_eq!(r["calendars"][2]["errors"][0]["reason"], "notFound");
    for bad in [
        "{}".to_string(),
        q.to_string()
            .replace("\"schema_version\":1", "\"schema_version\":2"),
        q.to_string().replace("\"time_zone\":", "\"typo\":"),
        q.to_string()
            .replace("\"primary\",", "\"primary\",\"primary\","),
        q.to_string().replace(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
        ),
    ] {
        std::fs::write(&path, bad).unwrap();
        assert_eq!(
            h.cli(&[
                "--json",
                "calendar",
                "free-busy",
                "--account",
                &account,
                "--file",
                path.to_str().unwrap()
            ])
            .await
            .unwrap()
            .status,
            2
        );
    }
    let after = h.google.control().snapshot().await;
    assert_eq!(
        after.calendars["alpha@example.test"]["alpha@example.test"].events,
        before.calendars["alpha@example.test"]["alpha@example.test"].events
    );
    assert_eq!(
        after.calendars["alpha@example.test"]["alpha@example.test"]
            .notifications
            .len(),
        before.calendars["alpha@example.test"]["alpha@example.test"]
            .notifications
            .len()
    );
    assert_eq!(
        after
            .requests
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/calendar/v3/freeBusy")
            .map(|r| r.count)
            .sum::<u64>(),
        1
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_cli_conflict_keeps_remote_version_and_permission_guards_have_no_effects() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    h.google.control().put_event("alpha@example.test","alpha@example.test",json!({"id":"conflict001","summary":"Initial","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"},"organizer":{"email":"organizer@example.test","self":false},"attendees":[{"email":"alpha@example.test","self":true,"responseStatus":"needsAction"},{"email":"other@example.test","responseStatus":"accepted"}]})).await.unwrap();
    h.google
        .control()
        .set_calendar_role("alpha@example.test", "team-alpha@example.test", "reader")
        .await
        .unwrap();
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
    let catalog = h
        .cli(&["--json", "calendar", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let calendars = catalog["result"]["items"].as_array().unwrap();
    let primary = calendars.iter().find(|c| c["is_primary"] == true).unwrap()["id"]
        .as_str()
        .unwrap();
    let reader = calendars
        .iter()
        .find(|c| c["access_role"] == "reader")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let agenda = h
        .cli(&[
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
        .unwrap()
        .json()
        .unwrap();
    let event = agenda["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["provider_id"] == "conflict001")
        .unwrap();
    let mut external = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .events["conflict001"]
        .clone();
    external["summary"] = json!("External newer title");
    h.google
        .control()
        .put_event("alpha@example.test", "alpha@example.test", external)
        .await
        .unwrap();
    let remote = h.google.control().snapshot().await;
    let path = h.artifacts.join("conflict.json");
    std::fs::write(&path,json!({"schema_version":1,"action":"update","event_id":event["id"],"expected_etag":event["etag"],"scope":"single","notifications":"all","patch":{"summary":"Desired losing title"}}).to_string()).unwrap();
    let args = [
        "--json",
        "calendar",
        "change",
        "--account",
        &account,
        "--calendar",
        primary,
        "--request-id",
        "33c6b748-796e-46dd-96f7-1d0d87a60dba",
        "--file",
        path.to_str().unwrap(),
        "--wait",
    ];
    let output = h.cli(&args).await.unwrap();
    assert_eq!(
        output.status,
        5,
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let failure = output.json().unwrap();
    let operation = &failure["error"]["operation"];
    assert_eq!(operation["state"], "conflict");
    let id = operation["id"].as_str().unwrap();
    let shown = h
        .cli(&[
            "--json",
            "operation",
            "show",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        shown["result"]["error_code"],
        "calendar_precondition_failed"
    );
    assert_eq!(
        serde_json::from_str::<Value>(shown["result"]["desired_state_json"].as_str().unwrap())
            .unwrap()["patch"]["summary"],
        "Desired losing title"
    );
    let observed = get(&h, &account, primary, event["id"].as_str().unwrap()).await;
    assert_eq!(
        observed["etag"],
        remote.calendars["alpha@example.test"]["alpha@example.test"].events["conflict001"]["etag"]
    );
    assert_eq!(observed["summary"], "External newer title");
    assert_eq!(h.cli(&args).await.unwrap().status, 5);
    let current_etag = observed["etag"].clone();
    for (calendar, bad) in [
        (
            reader,
            json!({"action":"create","event":{"summary":"Reader cannot create","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"}}}),
        ),
        (
            primary,
            json!({"action":"update","event_id":event["id"],"expected_etag":current_etag,"patch":{"guests_can_modify":true}}),
        ),
        (
            primary,
            json!({"action":"update","event_id":event["id"],"expected_etag":current_etag,"patch":{"attendees":[{"email":"other@example.test"}]}}),
        ),
    ] {
        let mut bad = bad;
        bad["schema_version"] = json!(1);
        bad["scope"] = json!("single");
        bad["notifications"] = json!("all");
        std::fs::write(&path, bad.to_string()).unwrap();
        assert_eq!(
            h.cli(&[
                "--json",
                "calendar",
                "change",
                "--account",
                &account,
                "--calendar",
                calendar,
                "--request-id",
                "8d9ef215-7dfe-44b3-8757-6e6341f7d637",
                "--file",
                path.to_str().unwrap(),
                "--wait"
            ])
            .await
            .unwrap()
            .status,
            2
        );
    }
    let final_state = h.google.control().snapshot().await;
    for key in ["alpha@example.test", "team-alpha@example.test"] {
        let a = &final_state.calendars["alpha@example.test"][key];
        let b = &remote.calendars["alpha@example.test"][key];
        assert_eq!(a.events, b.events);
        assert_eq!(a.version, b.version);
        assert_eq!(a.notifications.len(), b.notifications.len());
    }
    assert_eq!(
        final_state
            .requests
            .iter()
            .filter(|r| r.method == "PATCH"
                && r.path == "/calendar/v3/calendars/alpha@example.test/events/conflict001")
            .map(|r| r.count)
            .sum::<u64>(),
        1
    );
    h.shutdown().await.unwrap();
}
