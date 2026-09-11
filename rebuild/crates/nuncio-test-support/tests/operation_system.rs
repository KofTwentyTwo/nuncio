#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "support/mail_change_system.rs"]
mod mail_change_system;
pub mod support;
use nuncio_proto::v2::*;
use nuncio_test_support::google::Seed;
use sha2::{Digest, Sha256};
use support::{
    auth::{begin, finish},
    system::SystemHarness,
};

fn input(account: &str) -> SaveDraftRequest {
    SaveDraftRequest {account_id:account.into(),draft_id:None,expected_version:None,content:Some(serde_json::from_value(serde_json::json!({"to":[{"address":"recipient@example.test"}],"subject":"Private draft","text":"A".repeat(100_000)})).unwrap())}
}

#[tokio::test]
async fn google_send_system_distinguishes_rejection_from_lost_acknowledgement_using_independent_effects(
) {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for (phase, action, state, count) in [
        (
            Phase::Before,
            FaultAction::Status {
                code: 401,
                retry_after_secs: None,
            },
            "applied",
            1,
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 400,
                retry_after_secs: None,
            },
            "failed",
            0,
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 429,
                retry_after_secs: Some(0),
            },
            "applied",
            1,
        ),
        (Phase::After, FaultAction::Disconnect, "applied", 1),
        (Phase::After, FaultAction::MalformedJson, "applied", 1),
        (Phase::After, FaultAction::TruncatedBody, "applied", 1),
        (
            Phase::After,
            FaultAction::Status {
                code: 500,
                retry_after_secs: None,
            },
            "applied",
            1,
        ),
    ] {
        let auth_rejection = matches!(&action, FaultAction::Status { code: 401, .. });
        let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
        let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
            .await
            .account_id
            .unwrap();
        let draft = h
            .mail()
            .save_draft(input(&account))
            .await
            .unwrap()
            .into_inner();
        h.google
            .control()
            .inject(Fault {
                method: "POST".into(),
                path: "/gmail/v1/users/me/messages/send".into(),
                account: Some("alpha@example.test".into()),
                call: None,
                phase,
                action,
            })
            .await;
        let request = SendDraftRequest {
            account_id: account.clone(),
            draft_id: draft.id,
            request_id: "6f2123ed-c345-47b8-85f7-cf9a8c6530d2".into(),
            expected_version: Some(1),
        };
        let before = h
            .authenticated()
            .get_status(GetStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .storage
            .unwrap()
            .revision;
        let enqueued = h
            .mail()
            .send_draft(request.clone())
            .await
            .unwrap()
            .into_inner();
        let identity = OperationRequest {
            account_id: account.clone(),
            operation_id: enqueued.id.clone(),
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        let op = loop {
            let op = h
                .operations()
                .get_operation(identity.clone())
                .await
                .unwrap()
                .into_inner();
            if op.state == state {
                break op;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "expected {state}, got {} / {:?}",
                op.state,
                op.error_code
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            count
        );
        let attempts = h
            .operations()
            .list_attempts(ListOperationAttemptsRequest {
                account_id: account.clone(),
                operation_id: op.id.clone(),
                page_size: 25,
                page_token: None,
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            attempts.items[0].outcome.as_deref(),
            Some(if state == "failed" {
                "rejected"
            } else {
                "applied"
            })
        );
        if phase == Phase::After {
            assert_eq!(attempts.items.len(), 2);
            assert_eq!(attempts.items[0].kind, "reconcile");
            assert_eq!(attempts.items[0].receipts[0].source, "positive_read");
            assert_eq!(attempts.items[1].outcome.as_deref(), Some("uncertain"));
        }
        if phase == Phase::Before && state == "applied" {
            assert_eq!(attempts.items.len(), 2);
            assert_eq!(attempts.items[0].kind, "dispatch");
            assert_eq!(attempts.items[1].outcome.as_deref(), Some("rejected"));
        }
        if auth_rejection {
            assert_eq!(
                h.google
                    .control()
                    .snapshot()
                    .await
                    .requests
                    .iter()
                    .filter(|r| r.path == "/token")
                    .map(|r| r.count)
                    .sum::<u64>(),
                2,
                "one initial code exchange and one refresh, with each send attempt journaled"
            );
        }
        let end = h
            .authenticated()
            .get_status(GetStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .storage
            .unwrap()
            .revision;
        let mut stream = h
            .authenticated()
            .watch_changes(WatchChangesRequest {
                after_revision: before,
            })
            .await
            .unwrap()
            .into_inner();
        let mut last = before;
        let mut operation_changes = 0;
        while last < end {
            let event = tokio::time::timeout(std::time::Duration::from_secs(3), stream.message())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(event.revision > last);
            last = event.revision;
            if event.kind == "operation" {
                assert_eq!(event.account_id.as_deref(), Some(account.as_str()));
                assert_eq!(event.resource_id.as_deref(), Some(op.id.as_str()));
                operation_changes += 1;
            }
        }
        assert_eq!(
            operation_changes,
            if state == "failed" { 3 } else { 5 },
            "every enqueue/start/finish is observable through the authenticated change stream"
        );
        drop(stream);
        assert_eq!(h.mail().send_draft(request).await.unwrap().into_inner(), op);
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            count
        );
        h.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn send_enqueue_rpc_is_authenticated_idempotent_and_inspectable_for_a_paused_account() {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let draft = h
        .mail()
        .save_draft(input(&account))
        .await
        .unwrap()
        .into_inner();
    h.accounts()
        .disconnect_account(AccountRequest {
            account_id: account.clone(),
        })
        .await
        .unwrap();
    let request = SendDraftRequest {
        account_id: account.clone(),
        draft_id: draft.id.clone(),
        request_id: "749dd1eb-c206-4d84-a922-af20a43af712".into(),
        expected_version: None,
    };
    assert_eq!(
        h.anonymous_mail()
            .send_draft(request.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let before_enqueue = h
        .authenticated()
        .get_status(GetStatusRequest {})
        .await
        .unwrap()
        .into_inner()
        .storage
        .unwrap()
        .revision;
    let mut pending = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let mut client = h.mail();
        let request = request.clone();
        pending.spawn(async move { client.send_draft(request).await.unwrap().into_inner() });
    }
    let operation = pending.join_next().await.unwrap().unwrap();
    while let Some(result) = pending.join_next().await {
        assert_eq!(result.unwrap(), operation);
    }
    assert_eq!(operation.state, "queued");
    assert_eq!(
        h.authenticated()
            .get_status(GetStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .storage
            .unwrap()
            .revision,
        before_enqueue + 1
    );
    let mut changes = h
        .authenticated()
        .watch_changes(WatchChangesRequest {
            after_revision: before_enqueue,
        })
        .await
        .unwrap()
        .into_inner();
    let event = changes.message().await.unwrap().unwrap();
    assert_eq!(event.kind, "operation");
    assert_eq!(event.account_id.as_deref(), Some(account.as_str()));
    assert_eq!(event.resource_id.as_deref(), Some(operation.id.as_str()));
    drop(changes);
    let identity = OperationRequest {
        account_id: account.clone(),
        operation_id: operation.id.clone(),
    };
    let resolution = ResolveOperationRequest {
        account_id: account.clone(),
        operation_id: operation.id.clone(),
        expected_version: operation.version,
        decision: Some(resolve_operation_request::Decision::Abandon(
            OperationAbandonment {
                reason: "Unresolved work is no longer wanted".into(),
            },
        )),
    };
    assert_eq!(
        h.anonymous_operations()
            .resolve_operation(resolution.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        h.operations()
            .resolve_operation(resolution)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition,
        "queued work should use safe cancellation"
    );
    let list = ListOperationsRequest {
        account_id: account.clone(),
        page_size: 1,
        page_token: None,
    };
    assert_eq!(
        h.anonymous_operations()
            .list_operations(list.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let listed = h
        .operations()
        .list_operations(list)
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, operation.id);
    assert_eq!(listed.revision, before_enqueue + 1);
    let history = ListOperationAttemptsRequest {
        account_id: account.clone(),
        operation_id: operation.id.clone(),
        page_size: 25,
        page_token: None,
    };
    assert_eq!(
        h.anonymous_operations()
            .list_attempts(history.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert!(h
        .operations()
        .list_attempts(history)
        .await
        .unwrap()
        .into_inner()
        .items
        .is_empty());
    assert_eq!(
        h.anonymous_operations()
            .get_operation(identity.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        h.anonymous_operations()
            .cancel_operation(identity.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let mut changed = request.clone();
    changed.expected_version = Some(1);
    assert_eq!(
        h.mail().send_draft(changed).await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    let mut edit = input(&account);
    edit.draft_id = Some(draft.id.clone());
    edit.expected_version = Some(1);
    edit.content.as_mut().unwrap().text = Some("Changed draft".into());
    h.mail().save_draft(edit).await.unwrap();
    assert_eq!(
        h.mail()
            .send_draft(request.clone())
            .await
            .unwrap()
            .into_inner(),
        operation
    );
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(
        h.operations()
            .get_operation(identity.clone())
            .await
            .unwrap()
            .into_inner(),
        operation
    );
    h.mail()
        .delete_draft(DeleteDraftRequest {
            account_id: account.clone(),
            draft_id: draft.id,
            expected_version: 2,
        })
        .await
        .unwrap();
    assert_eq!(
        h.mail().send_draft(request).await.unwrap().into_inner(),
        operation
    );
    assert_eq!(
        h.operations()
            .cancel_operation(identity)
            .await
            .unwrap()
            .into_inner()
            .state,
        "cancelled"
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn prepare_rpc_is_authenticated_account_scoped_offline_and_preserves_context_when_edited() {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let other = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let raw = include_str!("../fixtures/mime/reply-forward.eml")
        .replace('\n', "\r\n")
        .into_bytes();
    h.google
        .control()
        .add_message(
            "alpha@example.test",
            "prepare-001",
            "prepare-thread",
            raw,
            ["INBOX".into()].into(),
        )
        .await
        .unwrap();
    let mut system = h.authenticated();
    let run = system
        .start_sync(StartSyncRequest {
            account_id: account.clone(),
            full: false,
            fetch_message_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let state = system
                .get_sync_run(SyncRunRequest {
                    account_id: account.clone(),
                    run_id: run.id.clone(),
                })
                .await
                .unwrap()
                .into_inner();
            if state.state == "succeeded" {
                break;
            }
            assert!(!matches!(state.state.as_str(), "failed" | "cancelled"));
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let messages = h
        .mail()
        .list_messages(ListMailRequest {
            account_id: account.clone(),
            page_size: 100,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let message = messages
        .items
        .iter()
        .find(|m| m.provider_id == "prepare-001")
        .unwrap()
        .id
        .clone();
    let request = PrepareDraftRequest {
        account_id: account.clone(),
        message_id: message,
        kind: "reply_all".into(),
        body: Some("Reply".into()),
        to: vec![],
    };
    assert_eq!(
        h.anonymous_mail()
            .prepare_draft(request.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let mut wrong = request.clone();
    wrong.account_id = other;
    assert_eq!(
        h.mail().prepare_draft(wrong).await.unwrap_err().code(),
        tonic::Code::NotFound
    );
    h.stop_google().await.unwrap();
    let saved = h
        .mail()
        .prepare_draft(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        saved
            .content
            .as_ref()
            .unwrap()
            .to
            .iter()
            .map(|r| r.address.as_str())
            .collect::<Vec<_>>(),
        [
            "alice@example.test",
            "bob@example.test",
            "carol@example.test"
        ]
    );
    assert_eq!(
        saved.context.as_ref().unwrap().in_reply_to.as_deref(),
        Some("incoming@example.test")
    );
    let mut edit = SaveDraftRequest {
        account_id: account.clone(),
        draft_id: Some(saved.id.clone()),
        expected_version: Some(1),
        content: saved.content.clone(),
    };
    edit.content.as_mut().unwrap().text = Some("Edited reply".into());
    let edited = h.mail().save_draft(edit).await.unwrap().into_inner();
    assert_eq!(edited.context, saved.context);
    let mut invalid = request.clone();
    invalid.to.push(DraftRecipient {
        address: "extra@example.test".into(),
        name: None,
    });
    assert_eq!(
        h.mail().prepare_draft(invalid).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let forward = PrepareDraftRequest {
        kind: "forward".into(),
        to: vec![DraftRecipient {
            address: "next@example.test".into(),
            name: None,
        }],
        ..request
    };
    let forwarded = h.mail().prepare_draft(forward).await.unwrap().into_inner();
    assert_eq!(
        forwarded.attachments[0].parameters["charset"],
        "windows-1252"
    );
    assert_eq!(
        forwarded.attachments[0].sha256,
        format!("{:x}", Sha256::digest(b"caf\xe9"))
    );
    assert_eq!(forwarded.attachments[1].disposition, "inline");
    assert!(forwarded.context.as_ref().unwrap().thread_id.is_none());
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    for draft in [edited, forwarded] {
        assert_eq!(
            h.mail()
                .get_draft(DraftRequest {
                    account_id: account.clone(),
                    draft_id: draft.id.clone()
                })
                .await
                .unwrap()
                .into_inner(),
            draft
        );
    }
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}

fn upload_header(account: &str, draft: &str, data: &[u8]) -> DraftUploadChunk {
    DraftUploadChunk {
        part: Some(draft_upload_chunk::Part::Header(DraftUploadHeader {
            account_id: account.into(),
            draft_id: draft.into(),
            expected_version: 1,
            filename: Some("résumé.bin".into()),
            mime_type: "application/octet-stream".into(),
            parameters: Default::default(),
            content_id: None,
            disposition: "attachment".into(),
            byte_length: data.len() as u64,
            sha256: format!("{:x}", Sha256::digest(data)),
        })),
    }
}
fn upload_data(offset: u64, data: &[u8]) -> DraftUploadChunk {
    DraftUploadChunk {
        part: Some(draft_upload_chunk::Part::Data(DraftUploadData {
            offset,
            data: data.to_vec(),
        })),
    }
}

#[tokio::test]
async fn draft_upload_rpc_rejects_partial_corrupt_stale_and_unauthenticated_streams_and_stops_cleanly(
) {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let saved = h
        .mail()
        .save_draft(input(&account))
        .await
        .unwrap()
        .into_inner();
    let data = vec![0x8f; 300_000];
    let header = upload_header(&account, &saved.id, &data);
    assert_eq!(
        h.anonymous_mail()
            .upload_draft_attachment(tokio_stream::iter(vec![header.clone()]))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let invalid = vec![
        vec![],
        vec![upload_data(0, &data[..262144])],
        vec![header.clone()],
        vec![header.clone(), header.clone()],
        vec![header.clone(), upload_data(1, &data[..262144])],
        vec![header.clone(), upload_data(0, &data[..262145])],
        vec![header.clone(), upload_data(0, &data[..100])],
        vec![
            header.clone(),
            upload_data(0, &data[..262144]),
            upload_data(262144, &vec![0; 37856]),
        ],
    ];
    for stream in invalid {
        assert_eq!(
            h.mail()
                .upload_draft_attachment(tokio_stream::iter(stream))
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
        assert_eq!(
            h.mail()
                .get_draft(DraftRequest {
                    account_id: account.clone(),
                    draft_id: saved.id.clone()
                })
                .await
                .unwrap()
                .into_inner(),
            saved
        );
    }
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    sender.send(header.clone()).await.unwrap();
    let mut client = h.mail();
    let pending = tokio::spawn(async move {
        client
            .upload_draft_attachment(tokio_stream::wrappers::ReceiverStream::new(receiver))
            .await
    });
    sender.send(upload_data(0, &data[..262144])).await.unwrap();
    let mut edit = input(&account);
    edit.draft_id = Some(saved.id.clone());
    edit.expected_version = Some(1);
    let edited = h.mail().save_draft(edit).await.unwrap().into_inner();
    sender
        .send(upload_data(262144, &data[262144..]))
        .await
        .unwrap();
    drop(sender);
    assert_eq!(
        pending.await.unwrap().unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: account.clone(),
                draft_id: saved.id.clone()
            })
            .await
            .unwrap()
            .into_inner(),
        edited
    );

    // An empty attachment is valid, while EOF before the promised length is not.
    let mut empty = upload_header(&account, &saved.id, &[]);
    if let Some(draft_upload_chunk::Part::Header(h)) = &mut empty.part {
        h.expected_version = 2;
    }
    let attached = h
        .mail()
        .upload_draft_attachment(tokio_stream::iter(vec![empty]))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(attached.version, 3);
    assert_eq!(attached.attachments.len(), 1);
    assert_eq!(attached.attachments[0].byte_length, 0);
    assert_eq!(
        attached.attachments[0].sha256,
        format!("{:x}", Sha256::digest([]))
    );

    let mut held_header = upload_header(&account, &saved.id, &data);
    if let Some(draft_upload_chunk::Part::Header(h)) = &mut held_header.part {
        h.expected_version = 3;
    }
    let (held_sender, receiver) = tokio::sync::mpsc::channel(1);
    held_sender.send(held_header).await.unwrap();
    let mut client = h.mail();
    let held = tokio::spawn(async move {
        client
            .upload_draft_attachment(tokio_stream::wrappers::ReceiverStream::new(receiver))
            .await
    });
    held_sender
        .send(upload_data(0, &data[..262144]))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), h.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(held.await.unwrap().is_err());
    drop(held_sender);
    h.restart().await.unwrap();
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: account.clone(),
                draft_id: saved.id
            })
            .await
            .unwrap()
            .into_inner(),
        attached
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}
#[tokio::test]
async fn draft_rpc_requires_authentication_and_preserves_concurrent_edits_across_restart() {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let other = finish(&h, &begin(&h, "beta@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let mut anon = h.anonymous_mail();
    assert_eq!(
        anon.save_draft(input(&account)).await.unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.get_draft(DraftRequest {
            account_id: account.clone(),
            draft_id: "unknown".into()
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.list_drafts(ListDraftsRequest {
            account_id: account.clone(),
            page_size: 100,
            page_token: None
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.delete_draft(DeleteDraftRequest {
            account_id: account.clone(),
            draft_id: "unknown".into(),
            expected_version: 1
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    let saved = h
        .mail()
        .save_draft(input(&account))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.version, 1);
    assert_eq!(
        saved.content.as_ref().unwrap().text.as_ref().unwrap().len(),
        100_000
    );
    let mut edit = input(&account);
    edit.draft_id = Some(saved.id.clone());
    edit.expected_version = Some(1);
    edit.content.as_mut().unwrap().subject = "Concurrent edit".into();
    let mut first = h.mail();
    let mut second = h.mail();
    let (one, two) = tokio::join!(first.save_draft(edit.clone()), second.save_draft(edit));
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    let result = match (one, two) {
        (Ok(result), Err(error)) | (Err(error), Ok(result)) => {
            assert_eq!(error.code(), tonic::Code::FailedPrecondition);
            result.into_inner()
        }
        _ => unreachable!(),
    };
    assert_eq!(result.version, 2);
    let mut invalid = input(&account);
    invalid.content.as_mut().unwrap().to[0].name =
        Some("Injected\r\nBcc: attacker@example.test".into());
    assert_eq!(
        h.mail().save_draft(invalid).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let mut large = input(&account);
    large.content.as_mut().unwrap().text = Some("x".repeat(1024 * 1024 + 1));
    assert_eq!(
        h.mail().save_draft(large).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: other.clone(),
                draft_id: saved.id.clone()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::NotFound
    );
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: account.clone(),
                draft_id: saved.id.clone()
            })
            .await
            .unwrap()
            .into_inner(),
        result
    );
    let listed = h
        .mail()
        .list_drafts(ListDraftsRequest {
            account_id: account.clone(),
            page_size: 1,
            page_token: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, saved.id);
    assert_eq!(
        h.mail()
            .delete_draft(DeleteDraftRequest {
                account_id: account.clone(),
                draft_id: saved.id.clone(),
                expected_version: 1
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    h.mail()
        .delete_draft(DeleteDraftRequest {
            account_id: account.clone(),
            draft_id: saved.id.clone(),
            expected_version: 2,
        })
        .await
        .unwrap();
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: account.clone(),
                draft_id: saved.id
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::NotFound
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}
