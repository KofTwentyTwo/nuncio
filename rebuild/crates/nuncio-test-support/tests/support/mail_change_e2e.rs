use super::{E2eHarness, Seed};
use serde_json::json;

#[tokio::test]
async fn mail_mutation_crashes_reconcile_remote_labels_and_preserve_concurrent_external_changes() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for boundary in [
        "operation_after_attempt",
        "operation_before_receipt",
        "remote_ack",
    ] {
        let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
        let account = h.connect_google("alpha@example.test").await.unwrap();
        assert_eq!(
            h.cli(&["--json", "sync", "--account", &account, "--wait"])
                .await
                .unwrap()
                .status,
            0
        );
        let list = h
            .cli(&["--json", "mail", "list", "--account", &account])
            .await
            .unwrap()
            .json()
            .unwrap();
        let message = list["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["provider_id"] == "m-001")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        if boundary == "remote_ack" {
            h.google
                .control()
                .inject(Fault {
                    method: "POST".into(),
                    path: "/gmail/v1/users/me/messages/m-001/modify".into(),
                    account: Some("alpha@example.test".into()),
                    call: None,
                    phase: Phase::After,
                    action: FaultAction::Withhold {
                        barrier: "mutation-accepted".into(),
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
        let file = h.artifacts.join("change-crash.json");
        std::fs::write(&file, br#"{"schema_version":1,"action":"archive"}"#).unwrap();
        let args = [
            "--json",
            "mail",
            "change",
            "--account",
            &account,
            "--message",
            message,
            "--request-id",
            "2e989d41-3c6f-4674-b1bb-d82fca7e198c",
            "--file",
            file.to_str().unwrap(),
        ];
        let queued = h.cli(&args).await.unwrap();
        assert_eq!(queued.status, 0);
        let queued = queued.json().unwrap();
        if boundary == "remote_ack" {
            h.google
                .control()
                .wait_for_barrier("mutation-accepted")
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
        let snapshot = h.google.control().snapshot().await;
        assert_eq!(
            snapshot.mail["alpha@example.test"].messages["m-001"]
                .labels
                .contains("INBOX"),
            boundary == "operation_after_attempt"
        );
        h.force_kill().await.unwrap();
        if boundary == "remote_ack" {
            h.google
                .control()
                .release_barrier("mutation-accepted")
                .await;
        }
        h.google
            .control()
            .change_labels(
                "alpha@example.test",
                "m-001",
                &["STARRED".into()],
                &["Label_project".into()],
            )
            .await
            .unwrap();
        h.restart().await.unwrap();
        let mut waited = args.to_vec();
        waited.push("--wait");
        let applied = h.cli(&waited).await.unwrap();
        assert_eq!(applied.status, 0, "{boundary}: {:?}", applied.json());
        assert_eq!(
            applied.json().unwrap()["result"]["id"],
            queued["result"]["id"]
        );
        let snapshot = h.google.control().snapshot().await;
        let remote = &snapshot.mail["alpha@example.test"].messages["m-001"].labels;
        assert_eq!(
            remote,
            &["STARRED".to_owned(), "UNREAD".to_owned()]
                .into_iter()
                .collect()
        );
        assert_eq!(
            snapshot
                .requests
                .iter()
                .filter(|r| r.method == "POST" && r.path.ends_with("/m-001/modify"))
                .map(|r| r.count)
                .sum::<u64>(),
            1
        );
        assert_eq!(snapshot.mail["alpha@example.test"].accepted_sends.len(), 0);
        assert_eq!(snapshot.mail["alpha@example.test"].message_copies, 0);
        let local = h
            .cli(&[
                "--json",
                "mail",
                "read",
                "--account",
                &account,
                "--message",
                message,
            ])
            .await
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(
            local["result"]["message"]["collections"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["provider_id"].as_str().unwrap().to_owned())
                .collect::<std::collections::BTreeSet<_>>(),
            *remote
        );
        let attempts = h
            .cli(&[
                "--json",
                "operation",
                "attempts",
                "--account",
                &account,
                "--operation",
                queued["result"]["id"].as_str().unwrap(),
            ])
            .await
            .unwrap()
            .json()
            .unwrap();
        if boundary == "operation_after_attempt" {
            assert_eq!(attempts["result"]["items"].as_array().unwrap().len(), 3);
            assert_eq!(attempts["result"]["items"][1]["outcome"], "repeatable");
        } else {
            assert_eq!(attempts["result"]["items"].as_array().unwrap().len(), 2);
            assert_eq!(
                attempts["result"]["items"][0]["receipts"][0]["source"],
                "positive_read"
            );
        }
        h.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn gmail_typed_mutations_preserve_unrelated_labels_and_are_idempotent_through_actual_cli() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await
            .unwrap()
            .status,
        0
    );
    let list = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let message = list["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["provider_id"] == "m-001")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let collections = h
        .cli(&["--json", "mail", "collections", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let label = collections["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["provider_id"] == "Label_project")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let capabilities = h
        .cli(&["--json", "mail", "capabilities", "--account", &account])
        .await
        .unwrap();
    assert_eq!(
        capabilities.status, 0,
        "Google label capabilities must be available to thin clients"
    );
    let caps = capabilities.json().unwrap();
    assert_eq!(caps["result"]["placement_model"], "labels");
    assert_eq!(caps["result"]["move_copy"], false);
    let beta = h.google.control().snapshot().await.mail["beta@example.test"].messages["m-001"]
        .labels
        .clone();
    let file = h.artifacts.join("mail-change.json");
    for invalid in [
        json!({"action":"archive"}),
        json!({"schema_version":2,"action":"archive"}),
        json!({"schema_version":1,"action":"archive","extra":true}),
    ] {
        std::fs::write(&file, serde_json::to_vec(&invalid).unwrap()).unwrap();
        let rejected = h
            .cli(&[
                "--json",
                "mail",
                "change",
                "--account",
                &account,
                "--message",
                message,
                "--request-id",
                "b5c6fcbf-8c9a-440f-a34b-6a73fd3c48b3",
                "--file",
                file.to_str().unwrap(),
            ])
            .await
            .unwrap();
        assert_eq!(rejected.status, 2);
    }
    let operations = h
        .cli(&["--json", "operation", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    assert!(operations["result"]["items"].as_array().unwrap().is_empty());
    for (i, mut action, present, absent) in [
        (
            0,
            json!({"action":"read","read":true}),
            vec!["INBOX", "Label_project"],
            vec!["UNREAD"],
        ),
        (
            1,
            json!({"action":"read","read":false}),
            vec!["INBOX", "Label_project", "UNREAD"],
            vec![],
        ),
        (
            2,
            json!({"action":"star","starred":true}),
            vec!["INBOX", "Label_project", "UNREAD", "STARRED"],
            vec![],
        ),
        (
            3,
            json!({"action":"star","starred":false}),
            vec!["INBOX", "Label_project", "UNREAD"],
            vec!["STARRED"],
        ),
        (
            4,
            json!({"action":"archive"}),
            vec!["Label_project", "UNREAD"],
            vec!["INBOX"],
        ),
        (
            5,
            json!({"action":"trash","trashed":true}),
            vec!["Label_project", "UNREAD", "TRASH"],
            vec!["INBOX"],
        ),
        (
            6,
            json!({"action":"trash","trashed":false}),
            vec!["Label_project", "UNREAD"],
            vec!["TRASH"],
        ),
        (
            7,
            json!({"action":"label","collection_id":label,"present":false}),
            vec!["UNREAD"],
            vec!["Label_project"],
        ),
        (
            8,
            json!({"action":"label","collection_id":label,"present":true}),
            vec!["Label_project", "UNREAD"],
            vec![],
        ),
    ] {
        action["schema_version"] = json!(1);
        std::fs::write(&file, serde_json::to_vec(&action).unwrap()).unwrap();
        let request = format!("00000000-0000-4000-8000-{i:012}");
        let args = [
            "--json",
            "mail",
            "change",
            "--account",
            &account,
            "--message",
            message,
            "--request-id",
            &request,
            "--file",
            file.to_str().unwrap(),
            "--wait",
        ];
        let changed = h.cli(&args).await.unwrap();
        assert_eq!(changed.status, 0, "{action}: {:?}", changed.json());
        let op = changed.json().unwrap();
        assert_eq!(op["result"]["state"], "applied");
        let snapshot = h.google.control().snapshot().await;
        let remote = &snapshot.mail["alpha@example.test"].messages["m-001"];
        for label in present {
            assert!(remote.labels.contains(label), "missing {label}");
        }
        for label in absent {
            assert!(!remote.labels.contains(label), "unexpected {label}");
        }
        assert_eq!(
            snapshot.mail["beta@example.test"].messages["m-001"].labels,
            beta
        );
        let local = h
            .cli(&[
                "--json",
                "mail",
                "read",
                "--account",
                &account,
                "--message",
                message,
            ])
            .await
            .unwrap()
            .json()
            .unwrap();
        let local_labels: std::collections::BTreeSet<_> = local["result"]["message"]["collections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["provider_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(local_labels, remote.labels);
        let writes = |snapshot: &nuncio_test_support::google::Snapshot| {
            snapshot
                .requests
                .iter()
                .filter(|r| r.method == "POST" && r.path.starts_with("/gmail/"))
                .map(|r| r.count)
                .sum::<u64>()
        };
        let before = writes(&snapshot);
        let repeated = h.cli(&args).await.unwrap();
        assert_eq!(repeated.status, 0);
        assert_eq!(repeated.json().unwrap()["result"]["id"], op["result"]["id"]);
        assert_eq!(writes(&h.google.control().snapshot().await), before);
    }
    let snapshot = h.google.control().snapshot().await;
    assert_eq!(snapshot.mail["alpha@example.test"].accepted_sends.len(), 0);
    assert_eq!(snapshot.mail["alpha@example.test"].message_copies, 0);
    for endpoint in ["trash", "untrash"] {
        assert!(snapshot.requests.iter().any(|r| r.method == "POST"
            && r.path == format!("/gmail/v1/users/me/messages/m-001/{endpoint}")
            && r.count == 1));
    }
    h.shutdown().await.unwrap();
}
