use crate::support::{
    auth::{begin, finish},
    system::SystemHarness,
};
use nuncio_proto::v2::*;
use nuncio_test_support::{google::Seed, TestError};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tonic::Code;

fn content() -> DraftContent {
    DraftContent {
        to: vec![DraftRecipient {
            address: "recipient@example.test".into(),
            name: None,
        }],
        subject: "Valid local draft".into(),
        text: Some("Local body".into()),
        ..Default::default()
    }
}
fn upload(
    header: DraftUploadHeader,
    bytes: Vec<u8>,
) -> impl futures_util::Stream<Item = DraftUploadChunk> {
    futures_util::stream::iter([
        DraftUploadChunk {
            part: Some(draft_upload_chunk::Part::Header(header)),
        },
        DraftUploadChunk {
            part: Some(draft_upload_chunk::Part::Data(DraftUploadData {
                offset: 0,
                data: bytes,
            })),
        },
    ])
}
fn header(account: &str, draft: &str, bytes: &[u8]) -> DraftUploadHeader {
    DraftUploadHeader {
        account_id: account.into(),
        draft_id: draft.into(),
        expected_version: 1,
        filename: Some("../escape.txt".into()),
        mime_type: "application/octet-stream".into(),
        content_id: None,
        disposition: "attachment".into(),
        byte_length: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        parameters: Default::default(),
    }
}

#[tokio::test]
async fn hostile_draft_headers_and_streams_cannot_cross_accounts_or_write_attachment_paths(
) -> Result<(), TestError> {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let a = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let b = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let original = h
        .mail()
        .save_draft(SaveDraftRequest {
            account_id: a.clone(),
            draft_id: None,
            expected_version: None,
            content: Some(content()),
        })
        .await?
        .into_inner();
    let remote = serde_json::to_value(h.google.control().snapshot().await)?;
    let outside = h.directory.parent().unwrap().join("escape.txt");
    std::fs::write(&outside, b"original sentinel")?;
    let mut inputs = Vec::new();
    for attack in [
        "subject\r\nBcc: other@example.test",
        "subject\0suffix",
        "\u{1b}[2Jsubject",
    ] {
        let mut value = content();
        value.subject = attack.into();
        inputs.push(value);
    }
    for field in 0..3 {
        let mut value = content();
        let recipients = match field {
            0 => &mut value.to,
            1 => &mut value.cc,
            _ => &mut value.bcc,
        };
        recipients.push(DraftRecipient {
            address: "recipient@example.test\r\nBcc: other@example.test".into(),
            name: None,
        });
        inputs.push(value);
    }
    let mut value = content();
    value.to[0].name = Some("name\r\nBcc: other@example.test".into());
    inputs.push(value);
    for value in inputs {
        assert_eq!(
            h.mail()
                .save_draft(SaveDraftRequest {
                    account_id: a.clone(),
                    draft_id: Some(original.id.clone()),
                    expected_version: Some(1),
                    content: Some(value)
                })
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
    }
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: b.clone(),
                draft_id: original.id.clone()
            })
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );
    assert_eq!(
        h.mail()
            .delete_draft(DeleteDraftRequest {
                account_id: b.clone(),
                draft_id: original.id.clone(),
                expected_version: 1
            })
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );
    assert_eq!(
        h.mail()
            .send_draft(SendDraftRequest {
                account_id: b.clone(),
                draft_id: original.id.clone(),
                request_id: "7602014a-1cf1-4495-a064-83e2745f8f3f".into(),
                expected_version: Some(1)
            })
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );
    let bytes = b"inert attachment bytes".to_vec();
    assert_eq!(
        h.mail()
            .upload_draft_attachment(upload(header(&b, &original.id, &bytes), bytes.clone()))
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );
    let mut headers = Vec::new();
    let mut v = header(&a, &original.id, &bytes);
    v.filename = Some("bad\r\nContent-Type: text/html".into());
    headers.push(v);
    let mut v = header(&a, &original.id, &bytes);
    v.mime_type = "text/plain\r\nBcc: other@example.test".into();
    headers.push(v);
    let mut v = header(&a, &original.id, &bytes);
    v.content_id = Some("bad\r\nInjected: value".into());
    headers.push(v);
    let mut v = header(&a, &original.id, &bytes);
    v.parameters
        .insert("charset".into(), "utf-8\r\nInjected: value".into());
    headers.push(v);
    let mut v = header(&a, &original.id, &bytes);
    v.byte_length = 64 * 1024 * 1024 + 1;
    headers.push(v);
    for v in headers {
        assert_eq!(
            h.mail()
                .upload_draft_attachment(upload(v, bytes.clone()))
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
    }
    assert_eq!(
        h.mail()
            .upload_draft_attachment(upload(header(&a, &original.id, &bytes), vec![0; 262145]))
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: a.clone(),
                draft_id: original.id.clone()
            })
            .await?
            .into_inner(),
        original
    );
    let attached = h
        .mail()
        .upload_draft_attachment(upload(header(&a, &original.id, &bytes), bytes))
        .await?
        .into_inner();
    assert_eq!(attached.version, 2);
    assert_eq!(
        attached.attachments[0].filename.as_deref(),
        Some("../escape.txt")
    );
    assert_eq!(std::fs::read(&outside)?, b"original sentinel");
    h.shutdown().await?;
    h.restart().await?;
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: a,
                draft_id: original.id
            })
            .await?
            .into_inner(),
        attached
    );
    assert_eq!(std::fs::read(&outside)?, b"original sentinel");
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await)?,
        remote
    );
    h.shutdown().await?;
    Ok(())
}

async fn sync(h: &SystemHarness, account: &str) -> SyncRun {
    let mut run = h
        .authenticated()
        .start_sync(StartSyncRequest {
            account_id: account.into(),
            full: true,
            fetch_message_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    tokio::time::timeout(Duration::from_secs(10), async {
        while matches!(run.state.as_str(), "queued" | "running") {
            tokio::time::sleep(Duration::from_millis(10)).await;
            run = h
                .authenticated()
                .get_sync_run(SyncRunRequest {
                    account_id: account.into(),
                    run_id: run.id.clone(),
                })
                .await
                .unwrap()
                .into_inner();
        }
    })
    .await
    .unwrap();
    run
}
fn query(account: &str) -> ListMailRequest {
    ListMailRequest {
        account_id: account.into(),
        collection_id: None,
        query: None,
        page_size: 100,
        page_token: None,
    }
}
#[tokio::test]
async fn malformed_provider_identifiers_fail_without_replacing_the_previous_projection(
) -> Result<(), TestError> {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    assert_eq!(sync(&h, &account).await.state, "succeeded");
    let original = h.mail().list_messages(query(&account)).await?.into_inner();
    for id in [".".into(), "..".into(), "bad\0id".into(), "x".repeat(8193)] {
        h.google
            .control()
            .add_message(
                "alpha@example.test",
                &id,
                "malformed-thread",
                b"From: sender@example.test\r\nSubject: Malformed identifier\r\n\r\nBody\r\n"
                    .to_vec(),
                ["INBOX".into()].into_iter().collect(),
            )
            .await?;
        assert_eq!(sync(&h, &account).await.state, "failed");
        let after = h.mail().list_messages(query(&account)).await?.into_inner();
        assert_eq!(after.items, original.items);
        assert_eq!(after.coverage, original.coverage);
        h.google
            .control()
            .delete_message("alpha@example.test", &id)
            .await?;
    }
    let remote = h.google.control().snapshot().await;
    for request in remote.requests {
        if request.path.starts_with("/gmail/") {
            assert!(
                [
                    "/gmail/v1/users/me/profile",
                    "/gmail/v1/users/me/labels",
                    "/gmail/v1/users/me/messages",
                    "/gmail/v1/users/me/history",
                    "/gmail/v1/users/me/messages/m-001",
                    "/gmail/v1/users/me/messages/m-002",
                    "/gmail/v1/users/me/messages/m-003"
                ]
                .contains(&request.path.as_str()),
                "unexpected provider request path"
            );
        }
    }
    for mailbox in remote.mail.values() {
        assert!(mailbox.accepted_sends.is_empty());
        assert_eq!(mailbox.message_copies, 0);
    }
    for calendars in remote.calendars.values() {
        for calendar in calendars.values() {
            assert!(calendar.notifications.is_empty());
        }
    }
    h.shutdown().await?;
    h.restart().await?;
    assert_eq!(
        h.mail()
            .list_messages(query(&account))
            .await?
            .into_inner()
            .items,
        original.items
    );
    h.shutdown().await?;
    Ok(())
}
