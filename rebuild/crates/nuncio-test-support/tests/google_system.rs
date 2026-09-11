#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "support/calendar_cases.rs"]
mod calendar_cases;
#[path = "support/calendar_write_system.rs"]
mod calendar_write_system;
#[path = "support/scheduling_cases.rs"]
mod scheduling_cases;

#[tokio::test]
async fn committed_change_stream_replays_and_closes_with_daemon_shutdown() {
    use nuncio_proto::v2::WatchChangesRequest;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    assert_eq!(
        h.unauthenticated()
            .watch_changes(WatchChangesRequest { after_revision: 0 })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        h.authenticated()
            .watch_changes(WatchChangesRequest {
                after_revision: u64::MAX
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::OutOfRange
    );
    let mut stream = h
        .authenticated()
        .watch_changes(WatchChangesRequest { after_revision: 0 })
        .await
        .unwrap()
        .into_inner();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let change = tokio::time::timeout(std::time::Duration::from_secs(2), stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(change.account_id.as_deref(), Some(account.as_str()));
    assert_eq!(change.revision, 1);
    let status = h
        .authenticated()
        .get_status(nuncio_proto::v2::GetStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(!status.background_sync);
    assert_eq!(status.sync.len(), 2);
    assert!(status.sync.iter().all(|s| s.account_id == account
        && s.last_success_at_ms.is_none()
        && s.age_ms.is_none()
        && s.coverage_state == "unavailable"));
    drop(stream);
    let mut replay = h
        .authenticated()
        .watch_changes(WatchChangesRequest { after_revision: 0 })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        replay.message().await.unwrap().unwrap().revision,
        change.revision
    );
    h.shutdown().await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), replay.message())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
}
pub mod support;
use nuncio_proto::v2::{
    AccountRequest, BeginGoogleAuthRequest, GetAuthStatusRequest, ListAccountsRequest,
};

fn account_secret_names(h: &SystemHarness) -> Vec<String> {
    let bytes = std::fs::read(&h.secrets_file).unwrap();
    let values: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    values
        .as_object()
        .unwrap()
        .keys()
        .filter(|key| key.contains("/account/"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn credential_write_failures_compensate_and_disconnect_cleanup_survives_restart() {
    use std::sync::atomic::Ordering;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    h.secrets.fail_put.store(true, Ordering::SeqCst);
    let failed = finish(&h, &begin(&h, "alpha@example.test", None).await, 400).await;
    assert_eq!(failed.error_code.as_deref(), Some("credential_storage"));
    assert!(h
        .accounts()
        .list_accounts(ListAccountsRequest {})
        .await
        .unwrap()
        .into_inner()
        .accounts
        .is_empty());
    assert!(account_secret_names(&h).is_empty());
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    assert_eq!(account_secret_names(&h).len(), 1);
    h.secrets.fail_delete.store(true, Ordering::SeqCst);
    let replacement = finish(
        &h,
        &begin(&h, "alpha@example.test", Some(alpha.clone())).await,
        200,
    )
    .await;
    assert_eq!(replacement.account_id.as_deref(), Some(alpha.as_str()));
    assert_eq!(
        replacement.warning_code.as_deref(),
        Some("credential_cleanup_pending")
    );
    assert_eq!(account_secret_names(&h).len(), 2);
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(account_secret_names(&h).len(), 1);
    assert_eq!(
        h.accounts()
            .check_account(AccountRequest {
                account_id: alpha.clone()
            })
            .await
            .unwrap()
            .into_inner()
            .state,
        "connected"
    );
    h.secrets.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        h.accounts()
            .disconnect_account(AccountRequest {
                account_id: alpha.clone()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Internal
    );
    let paused = h
        .accounts()
        .list_accounts(ListAccountsRequest {})
        .await
        .unwrap()
        .into_inner()
        .accounts;
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].state, "disconnected");
    assert_eq!(
        account_secret_names(&h).len(),
        1,
        "failed secure-store deletion must remain pending"
    );
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    assert!(
        account_secret_names(&h).is_empty(),
        "startup must finish the durable cleanup intent"
    );
    let saved = h
        .accounts()
        .list_accounts(ListAccountsRequest {})
        .await
        .unwrap()
        .into_inner()
        .accounts;
    assert_eq!(saved[0].id, alpha);
    assert_eq!(saved[0].state, "disconnected");
    h.shutdown().await.unwrap();
}
use nuncio_proto::v2::{GetStatusRequest, ShutdownRequest};
use nuncio_test_support::google::Seed;
use support::auth::{begin, browser, consent, finish};
use support::system::SystemHarness;

async fn sync_mail(h: &SystemHarness, account: &str, full: bool) -> nuncio_proto::v2::SyncRun {
    use nuncio_proto::v2::{StartSyncRequest, SyncRunRequest};
    let mut run = h
        .authenticated()
        .start_sync(StartSyncRequest {
            account_id: account.into(),
            full,
            fetch_message_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(run.state.as_str(), "queued" | "running") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "sync run did not finish"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        run = h
            .authenticated()
            .get_sync_run(SyncRunRequest {
                account_id: account.into(),
                run_id: run.id,
            })
            .await
            .unwrap()
            .into_inner();
    }
    run
}

#[tokio::test]
async fn missing_optional_content_and_payload_limits_remain_explicit_until_requested_fetch() {
    use nuncio_proto::v2::*;
    let mut h = SystemHarness::with_payload_limit(Seed::TwoAccounts, 4096)
        .await
        .unwrap();
    h.google
        .control()
        .omit_message_fields(
            "alpha@example.test",
            "m-003",
            &[
                "raw",
                "internalDate",
                "threadId",
                "historyId",
                "labelIds",
                "payload",
            ],
        )
        .await
        .unwrap();
    let mut big =
        b"From: sender@example.test\r\nSubject: Larger than configured limit\r\n\r\n".to_vec();
    big.extend(vec![b'x'; 8192]);
    h.google
        .control()
        .add_message(
            "alpha@example.test",
            "large-one",
            "large-thread",
            big,
            ["INBOX".into()].into_iter().collect(),
        )
        .await
        .unwrap();
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    assert_eq!(sync_mail(&h, &alpha, false).await.state, "succeeded");
    let items = h
        .mail()
        .list_messages(mail_query(&alpha))
        .await
        .unwrap()
        .into_inner()
        .items;
    let missing = items.iter().find(|m| m.provider_id == "m-003").unwrap();
    assert_eq!(missing.body_availability, "missing");
    assert!(missing.internal_date_ms.is_none());
    assert!(missing.thread_id.is_none());
    assert!(missing.history_id.is_none());
    assert!(missing.collections.is_empty());
    let large = items.iter().find(|m| m.provider_id == "large-one").unwrap();
    assert_eq!(large.body_availability, "too_large");
    for m in [missing, large] {
        assert_eq!(
            h.mail()
                .download(DownloadMailRequest {
                    account_id: alpha.clone(),
                    message_id: m.id.clone(),
                    kind: "raw".into(),
                    attachment_id: None
                })
                .await
                .unwrap_err()
                .code(),
            tonic::Code::NotFound
        );
    }
    h.google
        .control()
        .omit_message_fields("alpha@example.test", "m-003", &[])
        .await
        .unwrap();
    let run = h
        .authenticated()
        .start_sync(StartSyncRequest {
            account_id: alpha.clone(),
            full: false,
            fetch_message_id: Some(missing.id.clone()),
        })
        .await
        .unwrap()
        .into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let status = h
            .authenticated()
            .get_sync_run(SyncRunRequest {
                account_id: alpha.clone(),
                run_id: run.id.clone(),
            })
            .await
            .unwrap()
            .into_inner();
        if status.state == "succeeded" {
            break;
        }
        assert!(matches!(status.state.as_str(), "queued" | "running"));
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let found = h
        .mail()
        .get_message(MessageRequest {
            account_id: alpha.clone(),
            message_id: missing.id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(found.message.unwrap().body_availability, "available");
    let mut html = h
        .mail()
        .download(DownloadMailRequest {
            account_id: alpha.clone(),
            message_id: missing.id.clone(),
            kind: "html".into(),
            attachment_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    let mut bytes = vec![];
    while let Some(chunk) = html.message().await.unwrap() {
        bytes.extend(chunk.data)
    }
    assert!(String::from_utf8(bytes).unwrap().contains("<script>"));
    assert_eq!(
        h.mail()
            .list_messages(mail_query(&alpha))
            .await
            .unwrap()
            .into_inner()
            .items
            .len(),
        4,
        "explicit fetch must not replace the full projection"
    );
    h.shutdown().await.unwrap();
}
fn mail_query(account: &str) -> nuncio_proto::v2::ListMailRequest {
    nuncio_proto::v2::ListMailRequest {
        account_id: account.into(),
        collection_id: None,
        query: None,
        page_size: 100,
        page_token: None,
    }
}

#[tokio::test]
async fn initial_scan_catches_remote_changes_and_restarts_an_expired_generation() {
    use nuncio_proto::v2::*;
    use nuncio_test_support::google::{CursorScope, Fault, FaultAction, Phase};
    for cap in [1, 10] {
        for expire in [false, true] {
            let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
            h.google.control().set_page_cap(cap).await.unwrap();
            let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
                .await
                .account_id
                .unwrap();
            h.google
                .control()
                .inject(Fault {
                    method: "GET".into(),
                    path: "/gmail/v1/users/me/messages".into(),
                    account: Some("alpha@example.test".into()),
                    call: Some(1),
                    phase: Phase::After,
                    action: FaultAction::Withhold {
                        barrier: "listing-captured".into(),
                    },
                })
                .await;
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
            h.google
                .control()
                .wait_for_barrier("listing-captured")
                .await
                .unwrap();
            if expire {
                h.google
                    .control()
                    .expire_cursor("alpha@example.test", CursorScope::GmailHistory)
                    .await
                    .unwrap();
            }
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
            h.google.control().add_message("alpha@example.test","arrived-during-scan","new-thread",b"From: sender@example.test\r\nSubject: Arrived during scan\r\n\r\nCatch me in history\r\n".to_vec(),["INBOX".into()].into_iter().collect()).await.unwrap();
            h.google.control().release_barrier("listing-captured").await;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let status = h
                    .authenticated()
                    .get_sync_run(SyncRunRequest {
                        account_id: alpha.clone(),
                        run_id: run.id.clone(),
                    })
                    .await
                    .unwrap()
                    .into_inner();
                if status.state == "succeeded" {
                    break;
                }
                assert!(
                    matches!(status.state.as_str(), "queued" | "running"),
                    "{:?}",
                    status.error_code
                );
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let page = h
                .mail()
                .list_messages(mail_query(&alpha))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(page.items.len(), 3);
            assert!(page
                .items
                .iter()
                .any(|m| m.provider_id == "arrived-during-scan"));
            assert!(!page.items.iter().any(|m| m.provider_id == "m-002"));
            assert!(page
                .items
                .iter()
                .find(|m| m.provider_id == "m-001")
                .unwrap()
                .collections
                .iter()
                .any(|c| c.provider_id == "STARRED"));
            let remote = h.google.control().snapshot().await;
            assert_eq!(
                page.coverage.unwrap().cursor,
                Some(remote.mail["alpha@example.test"].history_id.clone())
            );
            assert_eq!(
                remote
                    .requests
                    .iter()
                    .find(|r| r.path == "/gmail/v1/users/me/profile")
                    .unwrap()
                    .count,
                if expire { 2 } else { 1 }
            );
            h.shutdown().await.unwrap();
        }
    }
}
#[tokio::test]
async fn gmail_projection_is_scoped_paged_and_full_reconciliation_never_promotes_failure() {
    use nuncio_proto::v2::*;
    use nuncio_test_support::google::{CursorScope, Fault, FaultAction, Phase};
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    h.google.control().set_page_overlap(true).await;
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let beta = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let mut anon = h.anonymous_mail();
    assert_eq!(
        anon.list_messages(mail_query(&alpha))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.get_message(MessageRequest {
            account_id: alpha.clone(),
            message_id: "missing".into()
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.list_collections(ListCollectionsRequest {
            account_id: alpha.clone()
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.download(DownloadMailRequest {
            account_id: alpha.clone(),
            message_id: "missing".into(),
            kind: "raw".into(),
            attachment_id: None
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    let mut anonymous = h.unauthenticated();
    assert_eq!(
        anonymous
            .start_sync(StartSyncRequest {
                account_id: alpha.clone(),
                full: false,
                fetch_message_id: None
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .get_sync_run(SyncRunRequest {
                account_id: alpha.clone(),
                run_id: "missing".into()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .cancel_sync(SyncRunRequest {
                account_id: alpha.clone(),
                run_id: "missing".into()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    for account in [&alpha, &beta] {
        let run = sync_mail(&h, account, false).await;
        assert_eq!(run.state, "succeeded", "{:?}", run.error_code);
    }
    let all = h
        .mail()
        .list_messages(mail_query(&alpha))
        .await
        .unwrap()
        .into_inner();
    let other = h
        .mail()
        .list_messages(mail_query(&beta))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(all.items.len(), 3);
    assert_eq!(other.items.len(), 3);
    let original = all.items.iter().find(|m| m.provider_id == "m-001").unwrap();
    assert_eq!(original.collections.len(), 3);
    assert_ne!(
        original.id,
        other
            .items
            .iter()
            .find(|m| m.provider_id == "m-001")
            .unwrap()
            .id
    );
    let mut q = mail_query(&alpha);
    q.page_size = 1;
    let first = h
        .mail()
        .list_messages(q.clone())
        .await
        .unwrap()
        .into_inner();
    q.page_token = first.next_page_token;
    assert!(q.page_token.is_some());
    let mut alien = q.clone();
    alien.account_id = beta.clone();
    assert_eq!(
        h.mail().list_messages(alien).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
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
    h.google
        .control()
        .delete_message("alpha@example.test", "m-002")
        .await
        .unwrap();
    let delta = sync_mail(&h, &alpha, false).await;
    assert_eq!(delta.mode, "delta");
    assert_eq!(delta.state, "succeeded");
    assert_eq!(
        h.mail().list_messages(q).await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    let current = h
        .mail()
        .list_messages(mail_query(&alpha))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(current.items.len(), 2);
    let changed = current
        .items
        .iter()
        .find(|m| m.provider_id == "m-001")
        .unwrap();
    assert_eq!(changed.id, original.id);
    assert!(changed
        .collections
        .iter()
        .any(|c| c.provider_id == "STARRED"));
    assert!(!changed
        .collections
        .iter()
        .any(|c| c.provider_id == "UNREAD"));
    // The first list page succeeds; the last page fails. Old complete state and
    // cursor must survive even though the provider has already deleted a row.
    h.google
        .control()
        .delete_message("alpha@example.test", "m-001")
        .await
        .unwrap();
    let raw = b"From: sender@example.test\r\nSubject: Added\r\n\r\nnew synthetic data\r\n".to_vec();
    h.google
        .control()
        .add_message(
            "alpha@example.test",
            "new-one",
            "new-thread",
            raw,
            ["INBOX".into()].into_iter().collect(),
        )
        .await
        .unwrap();
    let calls = h
        .google
        .control()
        .snapshot()
        .await
        .requests
        .iter()
        .find(|r| {
            r.account.as_deref() == Some("alpha@example.test")
                && r.path == "/gmail/v1/users/me/messages"
                && r.method == "GET"
        })
        .unwrap()
        .count;
    h.google
        .control()
        .inject(Fault {
            method: "GET".into(),
            path: "/gmail/v1/users/me/messages".into(),
            account: Some("alpha@example.test".into()),
            call: Some(calls + 2),
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 503,
                retry_after_secs: Some(2),
            },
        })
        .await;
    let failed = sync_mail(&h, &alpha, true).await;
    assert_eq!(failed.state, "failed");
    let retained = h
        .mail()
        .list_messages(mail_query(&alpha))
        .await
        .unwrap()
        .into_inner();
    assert!(retained.revision > current.revision);
    let mut progress = h
        .authenticated()
        .watch_changes(WatchChangesRequest {
            after_revision: current.revision,
        })
        .await
        .unwrap()
        .into_inner();
    let mut retry_changes = 0;
    for revision in current.revision + 1..=retained.revision {
        let change = tokio::time::timeout(std::time::Duration::from_secs(2), progress.message())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(change.revision, revision);
        assert_eq!(change.account_id.as_deref(), Some(alpha.as_str()));
        if change.kind == "sync_schedule" {
            assert_eq!(change.resource_id.as_deref(), Some("gmail"));
            retry_changes += 1;
        } else {
            assert_eq!(change.kind, "sync_run");
            assert_eq!(change.resource_id.as_deref(), Some(failed.id.as_str()));
        }
    }
    assert_eq!(
        retry_changes, 1,
        "provider deadline must commit exactly one schedule revision"
    );
    drop(progress);
    let mut expected = current.clone();
    expected.revision = retained.revision;
    assert_eq!(
        retained, expected,
        "Failed synchronization changed the complete mail projection or coverage"
    );
    assert!(retained.items.iter().any(|m| m.id == original.id));
    h.google
        .control()
        .expire_cursor("alpha@example.test", CursorScope::GmailHistory)
        .await
        .unwrap();
    h.google.control().reverse_results(true).await;
    let recovered = sync_mail(&h, &alpha, false).await;
    assert_eq!(recovered.state, "succeeded");
    assert_eq!(recovered.mode, "full");
    let final_mail = h
        .mail()
        .list_messages(mail_query(&alpha))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(final_mail.items.len(), 2);
    assert!(!final_mail.items.iter().any(|m| m.id == original.id));
    assert!(final_mail.items.iter().any(|m| m.provider_id == "new-one"));
    assert_eq!(
        h.mail()
            .list_messages(mail_query(&beta))
            .await
            .unwrap()
            .into_inner()
            .items
            .len(),
        3
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn lost_rotated_refresh_ack_requires_reauthorization_without_affecting_other_account() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let beta = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    h.google.control().rotate_refresh_tokens(true).await;
    h.google
        .control()
        .advance(std::time::Duration::from_secs(3601))
        .await;
    h.google
        .control()
        .inject(Fault {
            method: "POST".into(),
            path: "/token".into(),
            account: None,
            call: None,
            phase: Phase::After,
            action: FaultAction::Disconnect,
        })
        .await;
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
    assert_eq!(
        h.accounts()
            .check_account(AccountRequest {
                account_id: alpha.clone()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let accounts = h
        .accounts()
        .list_accounts(ListAccountsRequest {})
        .await
        .unwrap()
        .into_inner()
        .accounts;
    assert!(accounts
        .iter()
        .any(|a| a.id == alpha && a.state == "needs_auth"));
    assert_eq!(
        h.accounts()
            .check_account(AccountRequest { account_id: beta })
            .await
            .unwrap()
            .into_inner()
            .state,
        "connected"
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn pending_oauth_sessions_and_partial_browser_headers_do_not_prevent_shutdown() {
    use tokio::io::AsyncWriteExt;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let mut peers = Vec::new();
    for _ in 0..16 {
        let session = begin(&h, "alpha@example.test", None).await;
        let url = url::Url::parse(&session.browser_url).unwrap();
        let redirect = url::Url::parse(
            &url.query_pairs()
                .find(|(k, _)| k == "redirect_uri")
                .unwrap()
                .1,
        )
        .unwrap();
        let address = format!(
            "{}:{}",
            redirect.host_str().unwrap(),
            redirect.port().unwrap()
        );
        let mut peer = tokio::net::TcpStream::connect(address).await.unwrap();
        peer.write_all(b"GET /oauth/callback HTTP/1.1\r\nHost: ")
            .await
            .unwrap();
        peers.push(peer);
    }
    let error = h
        .accounts()
        .begin_google_auth(BeginGoogleAuthRequest {
            client_id: "nuncio-test-client".into(),
            client_secret: None,
            login_hint: None,
            account_id: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::ResourceExhausted);
    h.shutdown().await.unwrap();
    drop(peers);
}
#[tokio::test]
async fn oauth_accounts_are_authenticated_isolated_and_refreshes_are_serialized() {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let mut anonymous = h.anonymous_accounts();
    assert_eq!(
        anonymous
            .list_accounts(ListAccountsRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .begin_google_auth(BeginGoogleAuthRequest::default())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .get_auth_status(GetAuthStatusRequest::default())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .disconnect_account(AccountRequest::default())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .check_account(AccountRequest::default())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let beta = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    assert_ne!(alpha, beta);
    let before = h
        .google
        .control()
        .snapshot()
        .await
        .requests
        .into_iter()
        .filter(|r| r.path == "/token")
        .map(|r| r.count)
        .sum::<u64>();
    h.google
        .control()
        .advance(std::time::Duration::from_secs(3601))
        .await;
    h.google.control().rotate_refresh_tokens(true).await;
    let mut one = h.accounts();
    let mut two = h.accounts();
    let (a, b) = tokio::join!(
        one.check_account(AccountRequest {
            account_id: alpha.clone()
        }),
        two.check_account(AccountRequest {
            account_id: alpha.clone()
        })
    );
    assert_eq!(a.unwrap().into_inner().address, "alpha@example.test");
    assert_eq!(b.unwrap().into_inner().address, "alpha@example.test");
    let after = h
        .google
        .control()
        .snapshot()
        .await
        .requests
        .into_iter()
        .filter(|r| r.path == "/token")
        .map(|r| r.count)
        .sum::<u64>();
    assert_eq!(
        after - before,
        1,
        "concurrent checks must perform only one refresh"
    );
    h.google
        .control()
        .advance(std::time::Duration::from_secs(3601))
        .await;
    assert_eq!(
        h.accounts()
            .check_account(AccountRequest {
                account_id: alpha.clone()
            })
            .await
            .unwrap()
            .into_inner()
            .state,
        "connected"
    );
    h.google.control().revoke("alpha@example.test").await;
    assert_eq!(
        h.accounts()
            .check_account(AccountRequest {
                account_id: alpha.clone()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        h.accounts()
            .check_account(AccountRequest { account_id: beta })
            .await
            .unwrap()
            .into_inner()
            .address,
        "beta@example.test"
    );
    let mismatch = finish(
        &h,
        &begin(&h, "beta@example.test", Some(alpha.clone())).await,
        400,
    )
    .await;
    assert_eq!(mismatch.error_code.as_deref(), Some("identity_mismatch"));
    let reconnected = finish(
        &h,
        &begin(&h, "alpha@example.test", Some(alpha.clone())).await,
        200,
    )
    .await;
    assert_eq!(reconnected.account_id.as_deref(), Some(alpha.as_str()));
    assert_eq!(
        h.accounts()
            .list_accounts(ListAccountsRequest {})
            .await
            .unwrap()
            .into_inner()
            .accounts
            .len(),
        2
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn oauth_rejects_state_host_pkce_denial_expired_code_and_missing_scopes() {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let session = begin(&h, "alpha@example.test", None).await;
    let callback = consent(&session.browser_url).await;
    let mut wrong = callback.clone();
    wrong.query_pairs_mut().append_pair("state", "attacker");
    assert_eq!(browser().get(wrong).send().await.unwrap().status(), 400);
    let mut wrong = callback.clone();
    let fields: Vec<_> = wrong
        .query_pairs()
        .map(|(k, v)| {
            let v = if k == "state" {
                "wrong-state".into()
            } else {
                v.into_owned()
            };
            (k.into_owned(), v)
        })
        .collect();
    wrong.set_query(None);
    wrong.query_pairs_mut().extend_pairs(fields);
    assert_eq!(browser().get(wrong).send().await.unwrap().status(), 400);
    assert_eq!(
        browser()
            .get(callback.clone())
            .header("Host", "evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        h.accounts()
            .get_auth_status(GetAuthStatusRequest {
                session_id: session.session_id.clone()
            })
            .await
            .unwrap()
            .into_inner()
            .state,
        "pending"
    );
    h.google
        .control()
        .advance(std::time::Duration::from_secs(301))
        .await;
    assert_eq!(browser().get(callback).send().await.unwrap().status(), 400);
    assert_eq!(
        h.accounts()
            .get_auth_status(GetAuthStatusRequest {
                session_id: session.session_id
            })
            .await
            .unwrap()
            .into_inner()
            .error_code
            .as_deref(),
        Some("authorization_required")
    );
    let mut session = begin(&h, "alpha@example.test", None).await;
    let url = url::Url::parse(&session.browser_url).unwrap();
    let params: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let value = if k == "code_challenge" {
                "A".repeat(43)
            } else {
                v.into_owned()
            };
            (k.into_owned(), value)
        })
        .collect();
    let mut bad = url;
    bad.set_query(None);
    bad.query_pairs_mut().extend_pairs(params);
    session.browser_url = bad.into();
    assert_eq!(finish(&h, &session, 400).await.state, "failed");
    h.google.control().deny_consent(true).await;
    assert_eq!(
        finish(&h, &begin(&h, "alpha@example.test", None).await, 400)
            .await
            .state,
        "denied"
    );
    h.google.control().deny_consent(false).await;
    h.google
        .control()
        .deny_scope("https://www.googleapis.com/auth/calendar")
        .await;
    assert_eq!(
        finish(&h, &begin(&h, "alpha@example.test", None).await, 400)
            .await
            .error_code
            .as_deref(),
        Some("scope_denied")
    );
    assert!(h
        .accounts()
        .list_accounts(ListAccountsRequest {})
        .await
        .unwrap()
        .into_inner()
        .accounts
        .is_empty());
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn system_harness_uses_real_engine_store_and_authenticated_rpc() {
    let mut first = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let mut second = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let mut anonymous = first.unauthenticated();
    assert_eq!(
        anonymous
            .get_status(GetStatusRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .shutdown(ShutdownRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let mut client = first.authenticated();
    let status = client
        .get_status(GetStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(status.api_version, "nuncio.v2");
    assert_eq!(status.storage.unwrap().schema_version, 22);
    let other = second
        .authenticated()
        .get_status(GetStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_ne!(status.profile_id, other.profile_id);
    assert_eq!(first.google.control().snapshot().await.mail.len(), 2);
    first.shutdown().await.unwrap();
    assert!(client.get_status(GetStatusRequest {}).await.is_err());
    second.shutdown().await.unwrap();
}
