use super::{E2eHarness, Seed};
use nuncio_test_support::google::{Fault, FaultAction, Phase};
use serde_json::Value;
use std::time::Duration;

async fn wait_status(h: &E2eHarness, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let output = h.cli(&["--json", "system", "status"]).await.unwrap();
        assert_eq!(output.status, 0);
        let value = output.json().unwrap();
        assert!(
            value["result"]["scheduler_error"].is_null(),
            "scheduler failed: {value}"
        );
        if predicate(&value["result"]) {
            return value["result"].clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon scheduler did not converge: {value}"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
async fn gmail_calls(h: &E2eHarness, account: &str) -> u64 {
    h.google
        .control()
        .snapshot()
        .await
        .requests
        .iter()
        .filter(|r| r.account.as_deref() == Some(account) && r.path.starts_with("/gmail/"))
        .map(|r| r.count)
        .sum()
}

#[tokio::test]
async fn cli_wait_failures_include_an_inspectable_run_and_queued_work_remains_cancellable() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    h.google
        .control()
        .inject(Fault {
            method: "GET".into(),
            path: "/gmail/v1/users/me/profile".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 503,
                retry_after_secs: Some(6),
            },
        })
        .await;
    let failed = h
        .cli(&["--json", "sync", "--account", &account, "--wait"])
        .await
        .unwrap();
    assert_eq!(failed.status, 4);
    let failed = failed.json().unwrap();
    assert_eq!(failed["error"]["code"], "provider_unavailable");
    assert_eq!(failed["error"]["retryable"], true);
    let receipt = &failed["error"]["sync_run"];
    assert_eq!(
        receipt["account_id"], account,
        "failed wait must return its durable receipt"
    );
    assert_eq!(receipt["state"], "failed");
    let run_id = receipt["id"].as_str().unwrap();
    let inspected = h
        .cli(&[
            "--json",
            "system",
            "sync-status",
            "--account",
            &account,
            "--run",
            run_id,
        ])
        .await
        .unwrap();
    assert_eq!(inspected.status, 0);
    assert_eq!(&inspected.json().unwrap()["result"], receipt);
    let queued = h
        .cli(&["--json", "sync", "--account", &account])
        .await
        .unwrap();
    assert_eq!(queued.status, 0);
    let queued = queued.json().unwrap();
    assert_eq!(queued["result"]["state"], "queued");
    let id = queued["result"]["id"].as_str().unwrap();
    let cancelled = h
        .cli(&[
            "--json",
            "system",
            "cancel-sync",
            "--account",
            &account,
            "--run",
            id,
        ])
        .await
        .unwrap();
    assert_eq!(cancelled.status, 0);
    assert_eq!(cancelled.json().unwrap()["result"]["state"], "cancelled");
    assert_eq!(gmail_calls(&h, "alpha@example.test").await, 1);
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}
#[tokio::test]
async fn daemon_retains_backoff_after_crash_and_catches_up_after_simulated_wake() {
    let mut h = E2eHarness::start_with_polling(Seed::TwoAccounts, Some(60000))
        .await
        .unwrap();
    h.google
        .control()
        .inject(Fault {
            method: "GET".into(),
            path: "/gmail/v1/users/me/profile".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 503,
                retry_after_secs: Some(3600),
            },
        })
        .await;
    let alpha = h.connect_google("alpha@example.test").await.unwrap();
    let beta = h.connect_google("beta@example.test").await.unwrap();
    let failed =
        wait_status(&h, |v| {
            v["sync"].as_array().unwrap().iter().any(|s| {
                s["account_id"] == alpha && s["scope"] == "gmail" && s["phase"] == "backoff"
            })
        })
        .await;
    let retry_at = failed["sync"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["account_id"] == alpha && s["scope"] == "gmail")
        .unwrap()["next_attempt_at_ms"]
        .as_i64()
        .unwrap();
    assert!(retry_at >= 1772899200000);
    wait_status(&h, |v| {
        v["sync"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| {
                s["account_id"] == beta || s["account_id"] == alpha && s["scope"] == "calendar"
            })
            .all(|s| s["last_success_at_ms"].is_i64())
    })
    .await;
    assert_eq!(gmail_calls(&h, "alpha@example.test").await, 1);
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let saved =
        wait_status(&h, |v| {
            v["sync"].as_array().unwrap().iter().any(|s| {
                s["account_id"] == alpha && s["scope"] == "gmail" && s["phase"] == "backoff"
            })
        })
        .await;
    assert_eq!(
        saved["sync"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["account_id"] == alpha && s["scope"] == "gmail")
            .unwrap()["next_attempt_at_ms"],
        retry_at
    );
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(gmail_calls(&h, "alpha@example.test").await, 1);
    h.google
        .control()
        .change_labels("alpha@example.test", "m-001", &["STARRED".into()], &[])
        .await
        .unwrap();
    h.google.control().put_event("alpha@example.test","primary",serde_json::json!({"id":"alldate01","summary":"Changed while asleep","start":{"date":"2026-03-09"},"end":{"date":"2026-03-10"}})).await.unwrap();
    h.google.control().advance(Duration::from_secs(3602)).await;
    // Test-only control models suspend time that the running Instant did not count.
    std::fs::write(h.artifacts.join("barriers/clock-offset-ms"), "3602000").unwrap();
    let resumed = wait_status(&h, |v| {
        v["sync"].as_array().unwrap().iter().all(|s| {
            s["last_success_at_ms"]
                .as_i64()
                .is_some_and(|at| at >= retry_at)
        })
    })
    .await;
    assert!(resumed["sync"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["error_code"].is_null() && s["coverage_state"] == "current"));
    let mail = h
        .cli(&["--json", "mail", "list", "--account", &alpha])
        .await
        .unwrap();
    assert_eq!(mail.status, 0);
    assert!(mail.json().unwrap()["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["provider_id"] == "m-001"
            && m["collections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["provider_id"] == "STARRED")));
    let agenda = h
        .cli(&[
            "--json",
            "calendar",
            "agenda",
            "--account",
            &alpha,
            "--from",
            "2026-03-01",
            "--to",
            "2026-03-20",
        ])
        .await
        .unwrap();
    assert_eq!(agenda.status, 0);
    assert!(agenda.json().unwrap()["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["summary"] == "Changed while asleep"));
    for email in ["alpha@example.test", "beta@example.test"] {
        assert!(h.google.control().accepted_sends(email).await.is_empty());
    }
    assert!(h
        .google
        .control()
        .snapshot()
        .await
        .calendars
        .values()
        .flat_map(|c| c.values())
        .all(|c| c.notifications.is_empty()));
    h.shutdown().await.unwrap();
}
