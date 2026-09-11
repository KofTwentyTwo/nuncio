use super::super::backup_system::{backup, upload};
use super::*;
use crate::imap_effects::effects;
use serde_json::Value;

pub(super) async fn stable(
    h: &SystemHarness,
    account: &str,
    id: &str,
    expected: &str,
) -> Result<v2::Operation, TestError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
    loop {
        let op = h
            .operations()
            .get_operation(v2::OperationRequest {
                account_id: account.into(),
                operation_id: id.into(),
            })
            .await?
            .into_inner();
        if !op.needs_reconciliation
            && matches!(
                op.state.as_str(),
                "applied" | "uncertain" | "conflict" | "failed"
            )
        {
            assert_eq!(op.state, expected, "{:?}", op.error_code);
            return Ok(op);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} {:?}",
            op.state,
            op.error_code
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
async fn restore(
    h: &mut SystemHarness,
    mock: &MockMailPlus,
    bytes: &[u8],
    account: &str,
) -> Result<(), TestError> {
    let report = h
        .maintenance()
        .restore_backup(upload(bytes))
        .await?
        .into_inner();
    assert_eq!(report.held_operations, 1);
    h.shutdown().await?;
    h.directory = report.directory.into();
    h.restart().await?;
    let mut input = request(mock, true, "alpha@example.test").await?;
    input.account_id = Some(account.into());
    assert_eq!(
        h.accounts()
            .connect_imap(input)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id,
        account
    );
    Ok(())
}
pub(super) async fn reconcile(
    h: &SystemHarness,
    op: &v2::Operation,
    mode: v2::ReconciliationMode,
    state: &str,
) -> Result<(v2::ReconcileOperationRequest, v2::Operation), TestError> {
    let input = v2::ReconcileOperationRequest {
        account_id: op.account_id.clone(),
        operation_id: op.id.clone(),
        request_id: format!("00000000-0000-4000-8000-{:012x}", op.version),
        expected_version: op.version,
        mode: mode as i32,
    };
    h.operations().reconcile_operation(input.clone()).await?;
    let result = stable(h, &op.account_id, &op.id, state).await?;
    assert_eq!(result.request_id, op.request_id);
    assert_eq!(result.desired_state_json, op.desired_state_json);
    Ok((input, result))
}

#[tokio::test]
async fn restored_flag_intents_observe_and_resume_without_cross_account_or_duplicate_writes(
) -> Result<(), TestError> {
    for (star, present) in [(false, true), (false, false), (true, true), (true, false)] {
        for after in [false, true] {
            let mut mock = MockMailPlus::start(&[]).await?;
            let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
            let account = h
                .accounts()
                .connect_imap(request(&mock, true, "alpha@example.test").await?)
                .await?
                .into_inner()
                .account
                .unwrap()
                .id;
            let flag = if star { "\\Flagged" } else { "\\Seen" };
            let initial = if present {
                vec!["\\Answered"]
            } else {
                vec!["\\Answered", flag]
            };
            mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":initial,"remove":if present {vec![flag]} else {vec![]}})).await?;
            sync(&h, &mut mock, &account, true).await?;
            let message = list(&h, &account)
                .await?
                .items
                .into_iter()
                .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
                .unwrap();
            h.arm("operation_before_dispatch")?;
            let op = h
                .mail()
                .change_message(v2::ChangeMessageRequest {
                    account_id: account.clone(),
                    message_id: message.id.clone(),
                    request_id: "e657935d-4efc-45c2-90bb-b3c8460f78bd".into(),
                    action: Some(if star {
                        v2::change_message_request::Action::Star(v2::MailStarChange {
                            starred: present,
                        })
                    } else {
                        v2::change_message_request::Action::Read(v2::MailReadChange {
                            read: present,
                        })
                    }),
                })
                .await?
                .into_inner();
            h.wait("operation_before_dispatch").await?;
            let bytes = backup(&h).await;
            let initial = effects(&mut mock).await?;
            if after {
                mock.control(json!({"command":"inject","name":"lost-flag","protocol":"imap","verb":"UID STORE","phase":"after","action":"disconnect"})).await?;
                h.release("operation_before_dispatch")?;
                stable(&h, &account, &op.id, "applied").await?;
            }
            restore(&mut h, &mock, &bytes, &account).await?;
            let held = stable(&h, &account, &op.id, "uncertain").await?;
            let before = effects(&mut mock).await?;
            let (mut input, mut result) = reconcile(
                &h,
                &held,
                v2::ReconciliationMode::Observe,
                if after { "applied" } else { "uncertain" },
            )
            .await?;
            assert_eq!(effects(&mut mock).await?, before);
            if !after {
                assert_eq!(
                    result.error_code.as_deref(),
                    Some("reconciliation_resume_required")
                );
                (input, result) =
                    reconcile(&h, &result, v2::ReconciliationMode::ResumeSafe, "applied").await?;
            }
            let final_state = effects(&mut mock).await?;
            assert_eq!(final_state["writes"]["imap UID STORE"], json!([1, 1]));
            for verb in [
                "imap UID COPY",
                "imap UID MOVE",
                "imap UID EXPUNGE",
                "imap EXPUNGE",
                "imap APPEND",
                "smtp DATA",
            ] {
                assert_eq!(final_state["writes"][verb], json!([0, 0]));
            }
            assert_eq!(final_state["other"], initial["other"]);
            assert_eq!(final_state["deliveries"], initial["deliveries"]);
            let flags = final_state["mailboxes"]["INBOX"]["messages"][0]["flags"]
                .as_array()
                .unwrap();
            assert_eq!(flags.contains(&json!(flag)), present);
            assert!(flags.contains(&json!("\\Answered")));
            let local = h
                .mail()
                .get_message(v2::MessageRequest {
                    account_id: account.clone(),
                    message_id: message.id,
                })
                .await?
                .into_inner();
            let local_flags = serde_json::from_str::<Value>(&local.provider_json)?;
            assert_eq!(
                local_flags["flags"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect::<std::collections::BTreeSet<_>>(),
                flags.iter().map(|v| v.as_str().unwrap()).collect()
            );
            h.shutdown().await?;
            h.restart().await?;
            assert_eq!(
                serde_json::to_value(
                    h.operations()
                        .reconcile_operation(input)
                        .await?
                        .into_inner()
                )?,
                serde_json::to_value(result)?
            );
            assert_eq!(effects(&mut mock).await?, final_state);
            h.shutdown().await?;
            mock.shutdown().await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn restored_transfers_require_retained_copyuid_and_resume_only_proven_source_removal(
) -> Result<(), TestError> {
    for action in ["copy", "native_move", "fallback_move", "trash", "restore"] {
        for retained_copy in [false, true] {
            for original_finished in [false, true] {
                let hidden = if action == "native_move" {
                    vec![]
                } else {
                    vec!["MOVE"]
                };
                let mut mock = MockMailPlus::start(&hidden).await?;
                let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
                let account = h
                    .accounts()
                    .connect_imap(request(&mock, true, "alpha@example.test").await?)
                    .await?
                    .into_inner()
                    .account
                    .unwrap()
                    .id;
                mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":2,"add":["\\Deleted"],"remove":[]})).await?;
                sync(&h, &mut mock, &account, true).await?;
                let mut message = list(&h, &account)
                    .await?
                    .items
                    .into_iter()
                    .find(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0"))
                    .unwrap();
                if action == "restore" {
                    let trash = h
                        .mail()
                        .change_message(v2::ChangeMessageRequest {
                            account_id: account.clone(),
                            message_id: message.id,
                            request_id: "71166180-3b79-4a05-97bb-3e54f891f58b".into(),
                            action: Some(v2::change_message_request::Action::Trash(
                                v2::MailTrashChange { trashed: true },
                            )),
                        })
                        .await?
                        .into_inner();
                    stable(&h, &account, &trash.id, "applied").await?;
                    message = list(&h, &account)
                        .await?
                        .items
                        .into_iter()
                        .find(|m| m.collections[0].name.as_deref() == Some("Trash"))
                        .unwrap();
                }
                let source = if action == "restore" {
                    "Trash"
                } else {
                    "INBOX"
                };
                let destination = match action {
                    "trash" => "Trash",
                    "restore" => "INBOX",
                    _ => "Archive",
                };
                let original_raw=mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":source,"uid":1})).await?;
                let change = match action {
                    "copy" => {
                        let destination = h
                            .mail()
                            .list_collections(v2::ListCollectionsRequest {
                                account_id: account.clone(),
                            })
                            .await?
                            .into_inner()
                            .items
                            .into_iter()
                            .find(|c| c.name.as_deref() == Some("Archive"))
                            .unwrap()
                            .id;
                        v2::change_message_request::Action::Copy(v2::MailFolderChange {
                            destination_collection_id: destination,
                        })
                    }
                    "trash" | "restore" => {
                        v2::change_message_request::Action::Trash(v2::MailTrashChange {
                            trashed: action == "trash",
                        })
                    }
                    _ => v2::change_message_request::Action::Archive(v2::MailArchiveChange {}),
                };
                let initial = effects(&mut mock).await?;
                let checkpoint = if retained_copy {
                    "operation_after_imap_copy"
                } else {
                    "operation_before_dispatch"
                };
                h.arm(checkpoint)?;
                let op = h
                    .mail()
                    .change_message(v2::ChangeMessageRequest {
                        account_id: account.clone(),
                        message_id: message.id.clone(),
                        request_id: "148e8d12-df28-452b-bc21-f6d0ca673827".into(),
                        action: Some(change),
                    })
                    .await?
                    .into_inner();
                h.wait(checkpoint).await?;
                let bytes = backup(&h).await;
                if original_finished {
                    h.release(checkpoint)?;
                    stable(&h, &account, &op.id, "applied").await?;
                }
                restore(&mut h, &mock, &bytes, &account).await?;
                let held = stable(&h, &account, &op.id, "uncertain").await?;
                let before = effects(&mut mock).await?;
                let needs_removal = retained_copy
                    && !original_finished
                    && !matches!(action, "copy" | "native_move");
                let expected = if retained_copy && !needs_removal {
                    "applied"
                } else {
                    "uncertain"
                };
                let (mut input, mut result) =
                    reconcile(&h, &held, v2::ReconciliationMode::Observe, expected).await?;
                assert_eq!(
                    effects(&mut mock).await?,
                    before,
                    "observation changed remote {action}"
                );
                if result.state == "uncertain" {
                    assert_eq!(
                        result.error_code.as_deref(),
                        Some(if retained_copy {
                            "imap_source_removal_requires_resume"
                        } else {
                            "restored_imap_copy_identity_unknown"
                        })
                    );
                    (input, result) = reconcile(
                        &h,
                        &result,
                        v2::ReconciliationMode::ResumeSafe,
                        if retained_copy {
                            "applied"
                        } else {
                            "uncertain"
                        },
                    )
                    .await?;
                }
                let remote = effects(&mut mock).await?;
                let copied = retained_copy || original_finished;
                for verb in ["imap UID COPY", "imap UID MOVE"] {
                    let increment = u64::from(
                        copied && ((verb == "imap UID MOVE") == (action == "native_move")),
                    );
                    for index in 0..2 {
                        assert_eq!(remote["writes"][verb][index].as_u64().unwrap(),initial["writes"][verb][index].as_u64().unwrap()+increment,"duplicate copy: {action}, retained={retained_copy}, finished={original_finished}");
                    }
                }
                for verb in ["imap APPEND", "smtp DATA", "imap EXPUNGE"] {
                    assert_eq!(remote["writes"][verb], initial["writes"][verb]);
                }
                let removed = copied && action != "copy";
                for verb in ["imap UID STORE", "imap UID EXPUNGE"] {
                    for index in 0..2 {
                        assert_eq!(
                            remote["writes"][verb][index].as_u64().unwrap(),
                            initial["writes"][verb][index].as_u64().unwrap()
                                + u64::from(removed && action != "native_move")
                        );
                    }
                }
                let inbox = remote["mailboxes"]["INBOX"]["messages"].as_array().unwrap();
                let unrelated = inbox.iter().find(|m| m["uid"] == 2).unwrap();
                assert!(unrelated["flags"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("\\Deleted")));
                assert_eq!(
                    unrelated,
                    initial["mailboxes"]["INBOX"]["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|m| m["uid"] == 2)
                        .unwrap()
                );
                let targets = remote["mailboxes"][destination]["messages"]
                    .as_array()
                    .unwrap();
                assert_eq!(
                    targets.len(),
                    initial["mailboxes"][destination]["messages"]
                        .as_array()
                        .unwrap()
                        .len()
                        + usize::from(copied)
                );
                if copied {
                    let target = targets
                        .iter()
                        .find(|m| m["uid"] != 2 || destination != "INBOX")
                        .unwrap();
                    let raw=mock.control(json!({"command":"raw","account":"alpha@example.test","mailbox":destination,"uid":target["uid"]})).await?;
                    assert_eq!(raw["raw_base64"], original_raw["raw_base64"]);
                }
                assert_eq!(remote["other"], initial["other"]);
                assert_eq!(remote["deliveries"], initial["deliveries"]);
                if !retained_copy {
                    assert_eq!(remote, before);
                }
                let attempts = h
                    .operations()
                    .list_attempts(v2::ListOperationAttemptsRequest {
                        account_id: account.clone(),
                        operation_id: op.id.clone(),
                        page_size: 100,
                        page_token: None,
                    })
                    .await?
                    .into_inner()
                    .items;
                assert_eq!(
                    attempts.iter().filter(|a| a.kind == "dispatch").count(),
                    usize::from(retained_copy)
                );
                if retained_copy {
                    let original = attempts.iter().find(|a| a.ordinal == 1).unwrap();
                    assert_eq!(original.receipts.len(), 1);
                    assert_eq!(original.receipts[0].kind, "imap_copy");
                } else {
                    assert!(attempts.iter().all(|a| a.receipts.is_empty()));
                }
                h.shutdown().await?;
                h.restart().await?;
                assert_eq!(
                    serde_json::to_value(
                        h.operations()
                            .reconcile_operation(input)
                            .await?
                            .into_inner()
                    )?,
                    serde_json::to_value(result)?
                );
                assert_eq!(effects(&mut mock).await?, remote);
                h.shutdown().await?;
                mock.shutdown().await?;
            }
        }
    }
    Ok(())
}
