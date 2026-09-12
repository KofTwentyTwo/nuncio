use super::{begin, finish, Seed, SystemHarness};
use nuncio_proto::v2::*;
use nuncio_test_support::google::{Fault, FaultAction, Phase};
use std::time::Duration;

async fn status(h: &SystemHarness) -> GetStatusResponse {
    h.authenticated()
        .get_status(GetStatusRequest {})
        .await
        .unwrap()
        .into_inner()
}
async fn wait_status(
    h: &SystemHarness,
    predicate: impl Fn(&GetStatusResponse) -> bool,
) -> GetStatusResponse {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let s = status(h).await;
        if predicate(&s) {
            return s;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "scheduler state did not converge: {s:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn calls(h: &SystemHarness, account: &str, prefix: &str) -> u64 {
    h.google
        .control()
        .snapshot()
        .await
        .requests
        .iter()
        .filter(|r| r.account.as_deref() == Some(account) && r.path.starts_with(prefix))
        .map(|r| r.count)
        .sum()
}

#[tokio::test]
async fn oauth_retry_after_date_throttles_both_apis_and_account_checks_across_restart() {
    let mut h = SystemHarness::with_polling(Seed::TwoAccounts, 100)
        .await
        .unwrap();
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let initial = wait_status(&h, |s| {
        s.sync.len() == 2 && s.sync.iter().all(|s| s.last_success_at_ms.is_some())
    })
    .await;
    h.google
        .control()
        .inject(Fault {
            method: "POST".into(),
            path: "/token".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::Before,
            action: FaultAction::StatusWithRetryDate {
                code: 503,
                retry_after: "Sat, 07 Mar 2026 15:00:06 GMT".into(),
            },
        })
        .await;
    h.google.control().advance(Duration::from_secs(3601)).await;
    wait_status(&h, |s| {
        s.sync
            .iter()
            .any(|s| s.error_code.as_deref() == Some("unavailable"))
    })
    .await;
    let throttled = calls(&h, "alpha@example.test", "/token").await;
    assert!(
        throttled >= 2,
        "throttle must be observed on the independent token endpoint"
    );
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    let saved = status(&h).await;
    assert_eq!(saved.sync.len(), 2);
    assert!(saved
        .sync
        .iter()
        .all(|s| s.phase == "backoff" && s.next_attempt_at_ms == Some(1772895606000)));
    for _ in 0..2 {
        assert_eq!(
            h.accounts()
                .check_account(AccountRequest {
                    account_id: alpha.clone()
                })
                .await
                .unwrap_err()
                .code(),
            tonic::Code::Unavailable
        );
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
    assert_eq!(
        calls(&h, "alpha@example.test", "/token").await,
        throttled,
        "OAuth retry guidance was bypassed"
    );
    let restored = wait_status(&h, |s| {
        s.sync.iter().all(|s| {
            s.error_code.is_none()
                && s.last_success_at_ms
                    > initial
                        .sync
                        .iter()
                        .find(|old| old.scope == s.scope)
                        .unwrap()
                        .last_success_at_ms
        })
    })
    .await;
    assert!(restored.sync.iter().all(|s| s
        .last_success_at_ms
        .is_some_and(|at| at >= 1772895606000)
        && s.age_ms.is_some()
        && s.coverage_state == "current"));
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
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

#[tokio::test]
async fn polling_pauses_revoked_and_disconnected_accounts_then_resumes_after_reconnect() {
    let mut h = SystemHarness::with_polling(Seed::TwoAccounts, 100)
        .await
        .unwrap();
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let beta = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let initial = wait_status(&h, |s| {
        s.sync.len() == 4 && s.sync.iter().all(|s| s.last_success_at_ms.is_some())
    })
    .await;
    h.google.control().revoke("alpha@example.test").await;
    wait_status(&h, |s| {
        s.sync
            .iter()
            .filter(|s| s.account_id == alpha)
            .all(|s| s.phase == "paused")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stopped = calls(&h, "alpha@example.test", "/").await;
    let beta_before = calls(&h, "beta@example.test", "/gmail/").await;
    h.google
        .control()
        .change_labels("beta@example.test", "m-001", &["STARRED".into()], &[])
        .await
        .unwrap();
    h.google.control().put_event("beta@example.test", "primary", serde_json::json!({"id":"alldate01","summary":"Other account keeps polling","start":{"date":"2026-03-09"},"end":{"date":"2026-03-10"}})).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mail = h
            .mail()
            .list_messages(super::mail_query(&beta))
            .await
            .unwrap()
            .into_inner();
        let agenda = h
            .calendar()
            .list_agenda(ListAgendaRequest {
                account_id: beta.clone(),
                window: Some(AgendaWindow {
                    from: "2026-03-01".into(),
                    to: "2026-03-20".into(),
                }),
                page_size: 100,
                page_token: None,
            })
            .await
            .unwrap()
            .into_inner();
        if mail.items.iter().any(|m| {
            m.provider_id == "m-001" && m.collections.iter().any(|c| c.provider_id == "STARRED")
        }) && agenda
            .items
            .iter()
            .any(|e| e.summary.as_deref() == Some("Other account keeps polling"))
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        calls(&h, "alpha@example.test", "/").await,
        stopped,
        "revoked account retried while paused"
    );
    assert!(calls(&h, "beta@example.test", "/gmail/").await > beta_before);
    h.accounts()
        .disconnect_account(AccountRequest {
            account_id: beta.clone(),
        })
        .await
        .unwrap();
    wait_status(&h, |s| {
        s.sync
            .iter()
            .filter(|s| s.account_id == beta)
            .all(|s| s.phase == "paused")
    })
    .await;
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    let paused_requests = h.google.control().snapshot().await.requests;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await.requests).unwrap(),
        serde_json::to_value(paused_requests).unwrap()
    );
    for (email, id) in [("alpha@example.test", &alpha), ("beta@example.test", &beta)] {
        assert_eq!(
            finish(&h, &begin(&h, email, Some(id.clone())).await, 200)
                .await
                .account_id
                .as_deref(),
            Some(id.as_str())
        );
    }
    wait_status(&h, |s| {
        s.sync.iter().all(|s| {
            s.error_code.is_none()
                && s.phase != "paused"
                && s.last_success_at_ms
                    > initial
                        .sync
                        .iter()
                        .find(|old| old.account_id == s.account_id && old.scope == s.scope)
                        .unwrap()
                        .last_success_at_ms
        })
    })
    .await;
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    assert!(h
        .google
        .control()
        .accepted_sends("beta@example.test")
        .await
        .is_empty());
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

#[tokio::test]
async fn polling_honors_remote_retry_deadlines_across_restart_and_keeps_other_scopes_running() {
    let mut h = SystemHarness::with_polling(Seed::TwoAccounts, 100)
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
                code: 429,
                retry_after_secs: Some(6),
            },
        })
        .await;
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let beta = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let failed = wait_status(&h, |s| {
        s.sync.iter().any(|s| {
            s.account_id == alpha
                && s.scope == "gmail"
                && s.error_code.as_deref() == Some("unavailable")
        })
    })
    .await;
    let gmail = failed
        .sync
        .iter()
        .find(|s| s.account_id == alpha && s.scope == "gmail")
        .unwrap();
    let retry_at = gmail.next_attempt_at_ms.unwrap();
    assert_eq!(gmail.phase, "backoff");
    assert!(
        retry_at >= 1772895606000,
        "HTTP Retry-After must dominate exponential retry: {retry_at}"
    );
    assert!(gmail.last_success_at_ms.is_none());
    assert_eq!(gmail.coverage_state, "unavailable");
    wait_status(&h, |s| {
        s.sync
            .iter()
            .filter(|s| s.account_id == beta || s.account_id == alpha && s.scope == "calendar")
            .all(|s| s.last_success_at_ms.is_some())
    })
    .await;
    assert_eq!(calls(&h, "alpha@example.test", "/gmail/").await, 1);
    let before_beta = calls(&h, "beta@example.test", "/gmail/").await;
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    let restarted = status(&h).await;
    assert_eq!(
        restarted
            .sync
            .iter()
            .find(|s| s.account_id == alpha && s.scope == "gmail")
            .unwrap()
            .next_attempt_at_ms,
        Some(retry_at)
    );
    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert_eq!(
        calls(&h, "alpha@example.test", "/gmail/").await,
        1,
        "restart ignored provider guidance"
    );
    assert!(calls(&h, "beta@example.test", "/gmail/").await > before_beta);
    // Explicit sync queues and can be cancelled while the provider deadline remains pending.
    let run = h
        .authenticated()
        .start_sync(StartSyncRequest {
            account_id: alpha.clone(),
            full: false,
            fetch_message_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        calls(&h, "alpha@example.test", "/gmail/").await,
        1,
        "explicit sync bypassed retry guidance"
    );
    let resources = status(&h).await.resources.unwrap();
    assert!(
        resources.background_jobs >= 1,
        "provider-backoff waiters must retain job admission"
    );
    assert!(resources.background_jobs <= 64);
    let cancelled = h
        .authenticated()
        .cancel_sync(SyncRunRequest {
            account_id: alpha.clone(),
            run_id: run.id,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cancelled.state, "cancelled");
    wait_status(&h, |s| {
        s.sync
            .iter()
            .any(|s| s.account_id == alpha && s.scope == "gmail" && s.last_success_at_ms.is_some())
    })
    .await;
    let remote = h.google.control().snapshot().await;
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    assert!(h
        .google
        .control()
        .accepted_sends("beta@example.test")
        .await
        .is_empty());
    assert!(remote
        .calendars
        .values()
        .flat_map(|c| c.values())
        .all(|c| c.notifications.is_empty()));
    h.shutdown().await.unwrap();
}
