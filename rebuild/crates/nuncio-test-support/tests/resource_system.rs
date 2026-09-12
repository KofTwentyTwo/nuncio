#![allow(clippy::unwrap_used)]
pub mod support;

use nuncio_proto::v2::*;
use nuncio_test_support::{google::Seed, TestError};
use serde_json::json;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
use support::{
    auth::{begin, finish},
    system::SystemHarness,
};

#[tokio::test]
async fn ten_thousand_provider_messages_ingest_and_paginate_without_loss() -> Result<(), TestError>
{
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let control = h.google.control();
    let address = "alpha@example.test";
    control.set_page_cap(100).await?;
    let initial = control.snapshot().await.mail[address].messages.len();
    for index in initial..10_000 {
        let id = format!("resource-{index:05}");
        let raw=format!("From: sender@example.test\r\nSubject: Resource {index}\r\nMessage-ID: <resource-{index}@example.test>\r\n\r\nSynthetic body\r\n").into_bytes();
        control
            .add_message(address, &id, &id, raw, ["INBOX".into()].into())
            .await?;
        control.omit_message_fields(address, &id, &["raw"]).await?;
    }
    let expected: BTreeSet<_> = control.snapshot().await.mail[address]
        .messages
        .keys()
        .cloned()
        .collect();
    assert_eq!(expected.len(), 10_000);
    let account = finish(&h, &begin(&h, address, None).await, 200)
        .await
        .account_id
        .unwrap();
    let started = Instant::now();
    let mut run = h
        .authenticated()
        .start_sync(StartSyncRequest {
            account_id: account.clone(),
            full: true,
            fetch_message_id: None,
        })
        .await?
        .into_inner();
    tokio::time::timeout(Duration::from_secs(180), async {
        while matches!(run.state.as_str(), "queued" | "running") {
            tokio::time::sleep(Duration::from_millis(50)).await;
            run = h
                .authenticated()
                .get_sync_run(SyncRunRequest {
                    account_id: account.clone(),
                    run_id: run.id.clone(),
                })
                .await
                .unwrap()
                .into_inner();
        }
    })
    .await?;
    assert_eq!(run.state, "succeeded", "{:?}", run.error_code);
    assert_eq!(run.processed, 10_000);
    let sync_ms = started.elapsed().as_millis();
    let listed = Instant::now();
    let mut observed = BTreeSet::new();
    let mut page = None;
    let mut pages = 0;
    loop {
        let result = h
            .mail()
            .list_messages(ListMailRequest {
                account_id: account.clone(),
                page_size: 100,
                page_token: page,
                collection_id: None,
                query: None,
            })
            .await?
            .into_inner();
        assert!(result.items.len() <= 100);
        for row in result.items {
            if row.provider_id.starts_with("resource-") {
                assert_eq!(row.body_availability, "missing");
            }
            assert!(
                observed.insert(row.provider_id),
                "duplicate across API pages"
            );
        }
        pages += 1;
        page = result.next_page_token;
        if page.is_none() {
            break;
        }
    }
    assert_eq!(observed, expected);
    assert_eq!(pages, 100);
    let remote = control.snapshot().await;
    let message_pages: u64 = remote
        .requests
        .iter()
        .filter(|r| r.path == "/gmail/v1/users/me/messages" && r.method == "GET")
        .map(|r| r.count)
        .sum();
    assert_eq!(message_pages, 100);
    let resources = h
        .authenticated()
        .get_status(GetStatusRequest {})
        .await?
        .into_inner()
        .resources
        .unwrap();
    let history_pages: u64 = remote
        .requests
        .iter()
        .filter(|r| r.path == "/gmail/v1/users/me/history")
        .map(|r| r.count)
        .sum();
    assert_eq!(
        resources.storage_page_batches,
        message_pages + history_pages
    );
    assert_eq!(resources.requests_active, 0);
    assert!(resources.requests_peak <= 2);
    assert!(resources.bytes_received > 0);

    for mailbox in remote.mail.values() {
        assert!(mailbox.accepted_sends.is_empty());
        assert_eq!(mailbox.message_copies, 0);
    }
    for calendars in remote.calendars.values() {
        for calendar in calendars.values() {
            assert!(calendar.notifications.is_empty());
        }
    }
    if let Some(directory) = std::env::var_os("NUNCIO_TEST_ARTIFACTS") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("metadata-load.json"),
            serde_json::to_vec_pretty(
                &json!({"messages":observed.len(),"api_pages":pages,"remote_message_pages":message_pages,"sync_ms":sync_ms,"list_ms":listed.elapsed().as_millis(),"resources":resources,"rss":"measured separately in actual subprocess suite"}),
            )?,
        )?;
    }
    h.shutdown().await?;
    h.restart().await?;
    assert_eq!(
        h.mail()
            .list_messages(ListMailRequest {
                account_id: account,
                page_size: 100,
                ..Default::default()
            })
            .await?
            .into_inner()
            .items
            .len(),
        100
    );
    h.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn configured_provider_payload_bound_retains_metadata_and_refuses_large_body(
) -> Result<(), TestError> {
    let mut h = SystemHarness::with_payload_limit(Seed::TwoAccounts, 4096).await?;
    let control = h.google.control();
    let address = "alpha@example.test";
    let mut raw=b"From: sender@example.test\r\nSubject: Above configured bound\r\nContent-Type: text/plain\r\n\r\n".to_vec();
    raw.extend_from_slice(&vec![b'Z'; 8192]);
    control
        .add_message(
            address,
            "too-large",
            "too-large-thread",
            raw,
            ["INBOX".into()].into(),
        )
        .await?;
    let account = finish(&h, &begin(&h, address, None).await, 200)
        .await
        .account_id
        .unwrap();
    let remote = control.snapshot().await.mail[address].messages["too-large"]
        .raw
        .clone();
    let mut run = h
        .authenticated()
        .start_sync(StartSyncRequest {
            account_id: account.clone(),
            full: true,
            fetch_message_id: None,
        })
        .await?
        .into_inner();
    tokio::time::timeout(Duration::from_secs(15), async {
        while matches!(run.state.as_str(), "queued" | "running") {
            tokio::time::sleep(Duration::from_millis(10)).await;
            run = h
                .authenticated()
                .get_sync_run(SyncRunRequest {
                    account_id: account.clone(),
                    run_id: run.id.clone(),
                })
                .await
                .unwrap()
                .into_inner();
        }
    })
    .await?;
    assert_eq!(run.state, "succeeded");
    let result = h
        .mail()
        .list_messages(ListMailRequest {
            account_id: account.clone(),
            page_size: 100,
            ..Default::default()
        })
        .await?
        .into_inner();
    let row = result
        .items
        .iter()
        .find(|m| m.provider_id == "too-large")
        .unwrap();
    assert_eq!(row.body_availability, "too_large");
    assert_eq!(row.subject.as_deref(), Some("Above configured bound"));
    assert_eq!(
        h.mail()
            .download(DownloadMailRequest {
                account_id: account.clone(),
                message_id: row.id.clone(),
                kind: "raw".into(),
                attachment_id: None
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::NotFound
    );
    let after = control.snapshot().await;
    assert_eq!(after.mail[address].messages["too-large"].raw, remote);
    assert!(after.mail[address].accepted_sends.is_empty());
    assert_eq!(after.mail[address].message_copies, 0);
    h.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn authenticated_request_flood_is_rejected_at_admission_and_recovers() -> Result<(), TestError>
{
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let control = h.google.control();
    control
        .inject(Fault {
            method: "POST".into(),
            path: "/calendar/v3/freeBusy".into(),
            account: Some("alpha@example.test".into()),
            call: Some(1),
            phase: Phase::Before,
            action: FaultAction::Withhold {
                barrier: "resource-admission".into(),
            },
        })
        .await;
    let query = FreeBusyRequest {
        account_id: account,
        from: "2026-10-02T00:00:00Z".into(),
        to: "2026-10-04T00:00:00Z".into(),
        time_zone: "UTC".into(),
        provider_calendar_ids: vec!["primary".into()],
    };
    let mut client = h.calendar();
    let first_query = query.clone();
    let first = tokio::spawn(async move { client.query_free_busy(first_query).await });
    control.wait_for_barrier("resource-admission").await?;
    let mut waiting = tokio::task::JoinSet::new();
    for _ in 0..64 {
        let mut client = h.calendar();
        let q = query.clone();
        waiting.spawn(async move { client.query_free_busy(q).await });
    }
    let rejected = tokio::time::timeout(Duration::from_millis(750), waiting.join_next()).await?;
    assert_eq!(
        rejected.unwrap()?.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    control.release_barrier("resource-admission").await;
    assert!(first.await??.into_inner().complete);
    while let Some(result) = waiting.join_next().await {
        assert!(result??.into_inner().complete);
    }
    let before = control.snapshot().await;
    let requests: u64 = before
        .requests
        .iter()
        .filter(|r| r.path == "/calendar/v3/freeBusy")
        .map(|r| r.count)
        .sum();
    assert_eq!(requests, 64, "rejected request must not reach the provider");
    assert!(
        h.calendar()
            .query_free_busy(query)
            .await?
            .into_inner()
            .complete
    );
    let after = control.snapshot().await;
    assert_eq!(
        after
            .requests
            .iter()
            .filter(|r| r.path == "/calendar/v3/freeBusy")
            .map(|r| r.count)
            .sum::<u64>(),
        65
    );
    for mailbox in after.mail.values() {
        assert!(mailbox.accepted_sends.is_empty());
        assert_eq!(mailbox.message_copies, 0);
    }
    for calendars in after.calendars.values() {
        for calendar in calendars.values() {
            assert!(calendar.notifications.is_empty());
        }
    }
    h.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn queued_send_survives_full_request_admission_without_restart() -> Result<(), TestError> {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let draft = h
        .mail()
        .save_draft(SaveDraftRequest {
            account_id: account.clone(),
            content: Some(serde_json::from_value(json!({
                "to":[{"address":"recipient@example.test"}],
                "subject":"Admission recovery", "text":"Durable queued mail"
            }))?),
            ..Default::default()
        })
        .await?
        .into_inner();
    h.arm("operation_before_dispatch")?;
    h.arm("operation_job_finished")?;
    let operation = h
        .mail()
        .send_draft(SendDraftRequest {
            account_id: account.clone(),
            draft_id: draft.id,
            request_id: "d4423e88-c724-49bb-a75d-2e993dbfe449".into(),
            expected_version: Some(1),
        })
        .await?
        .into_inner();
    h.wait("operation_before_dispatch").await?;
    let control = h.google.control();
    control
        .inject(Fault {
            method: "POST".into(),
            path: "/calendar/v3/freeBusy".into(),
            account: Some("alpha@example.test".into()),
            call: Some(1),
            phase: Phase::Before,
            action: FaultAction::Withhold {
                barrier: "queued-send-admission".into(),
            },
        })
        .await;
    let query = FreeBusyRequest {
        account_id: account.clone(),
        from: "2026-10-02T00:00:00Z".into(),
        to: "2026-10-04T00:00:00Z".into(),
        time_zone: "UTC".into(),
        provider_calendar_ids: vec!["primary".into()],
    };
    let mut client = h.calendar();
    let first_query = query.clone();
    let first = tokio::spawn(async move { client.query_free_busy(first_query).await });
    control.wait_for_barrier("queued-send-admission").await?;
    let mut waiting = tokio::task::JoinSet::new();
    for _ in 0..64 {
        let mut client = h.calendar();
        let query = query.clone();
        waiting.spawn(async move { client.query_free_busy(query).await });
    }
    let rejected = tokio::time::timeout(Duration::from_millis(750), waiting.join_next()).await?;
    assert_eq!(
        rejected.unwrap()?.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    h.release("operation_before_dispatch")?;
    h.wait("operation_job_finished").await?;
    let attempts = || ListOperationAttemptsRequest {
        account_id: account.clone(),
        operation_id: operation.id.clone(),
        page_size: 25,
        page_token: None,
    };
    assert!(h
        .operations()
        .list_attempts(attempts())
        .await?
        .into_inner()
        .items
        .is_empty());
    assert!(control
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    control.release_barrier("queued-send-admission").await;
    assert!(first.await??.into_inner().complete);
    while let Some(result) = waiting.join_next().await {
        assert!(result??.into_inner().complete);
    }
    h.release("operation_job_finished")?;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = h
                .authenticated()
                .get_status(GetStatusRequest {})
                .await?
                .into_inner();
            assert_eq!(
                status.operation_worker_error, None,
                "admission refusal must not stop the worker"
            );
            let current = h
                .operations()
                .get_operation(OperationRequest {
                    account_id: account.clone(),
                    operation_id: operation.id.clone(),
                })
                .await?
                .into_inner();
            if current.state == "applied" {
                break Ok::<_, TestError>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let attempts = h.operations().list_attempts(attempts()).await?.into_inner();
    assert_eq!(attempts.items.len(), 1);
    assert_eq!(attempts.items[0].outcome.as_deref(), Some("applied"));
    let after = control.snapshot().await;
    assert_eq!(after.mail["alpha@example.test"].accepted_sends.len(), 1);
    assert!(after.mail["beta@example.test"].accepted_sends.is_empty());
    assert_eq!(
        after
            .requests
            .iter()
            .filter(|r| r.path == "/calendar/v3/freeBusy")
            .map(|r| r.count)
            .sum::<u64>(),
        64
    );
    for calendars in after.calendars.values() {
        for calendar in calendars.values() {
            assert!(calendar.notifications.is_empty());
        }
    }
    h.shutdown().await?;
    Ok(())
}
