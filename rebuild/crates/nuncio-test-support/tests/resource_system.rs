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
                &json!({"messages":observed.len(),"api_pages":pages,"remote_message_pages":message_pages,"sync_ms":sync_ms,"list_ms":listed.elapsed().as_millis(),"rss":"measured separately in actual subprocess suite"}),
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
