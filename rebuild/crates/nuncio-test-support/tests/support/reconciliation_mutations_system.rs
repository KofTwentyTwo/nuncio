use super::*;
use nuncio_test_support::google::Snapshot;

async fn sync_done(h: &SystemHarness, mut run: SyncRun) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(run.state.as_str(), "queued" | "running") {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        run = h
            .authenticated()
            .get_sync_run(SyncRunRequest {
                account_id: run.account_id,
                run_id: run.id,
            })
            .await
            .unwrap()
            .into_inner();
    }
    assert_eq!(run.state, "succeeded");
}
async fn restore(h: &mut SystemHarness, bytes: &[u8], account: &str) {
    let report = h
        .maintenance()
        .restore_backup(upload(bytes))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(report.held_operations, 1);
    h.shutdown().await.unwrap();
    h.directory = report.directory.into();
    h.restart().await.unwrap();
    assert_eq!(
        finish(h, &begin(h, "alpha@example.test", None).await, 200)
            .await
            .account_id
            .as_deref(),
        Some(account)
    );
}
async fn reconcile(
    h: &SystemHarness,
    op: &Operation,
    mode: ReconciliationMode,
    state: &str,
) -> (ReconcileOperationRequest, Operation) {
    let request = ReconcileOperationRequest {
        account_id: op.account_id.clone(),
        operation_id: op.id.clone(),
        request_id: format!("00000000-0000-4000-8000-{:012x}", op.version),
        expected_version: op.version,
        mode: mode as i32,
    };
    h.operations()
        .reconcile_operation(request.clone())
        .await
        .unwrap();
    let result = settled(h, &op.account_id, &op.id, state).await;
    assert_eq!(result.request_id, op.request_id);
    assert_eq!(result.desired_state_json, op.desired_state_json);
    (request, result)
}
fn same_effects(before: &Snapshot, after: &Snapshot) {
    assert_eq!(
        serde_json::to_value(&before.mail).unwrap(),
        serde_json::to_value(&after.mail).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&before.calendars).unwrap(),
        serde_json::to_value(&after.calendars).unwrap()
    );
    assert_eq!(writes(before), writes(after));
}
fn writes(state: &Snapshot) -> u64 {
    state
        .requests
        .iter()
        .filter(|r| {
            r.method != "GET" && (r.path.starts_with("/gmail/") || r.path.starts_with("/calendar/"))
        })
        .map(|r| r.count)
        .sum()
}
async fn replay_after_restart(
    h: &mut SystemHarness,
    request: ReconcileOperationRequest,
    op: &Operation,
) {
    let before = h.google.control().snapshot().await;
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    let replay = h
        .operations()
        .reconcile_operation(request)
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(op).unwrap()
    );
    same_effects(&before, &h.google.control().snapshot().await);
}

#[tokio::test]
async fn restored_gmail_mutations_observe_without_writes_and_resume_only_explicit_desired_state() {
    for action in ["read", "star", "archive", "trash", "restore", "label"] {
        for already_applied in [false, true] {
            let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
            let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
                .await
                .account_id
                .unwrap();
            if action == "restore" {
                h.google
                    .control()
                    .change_labels(
                        "alpha@example.test",
                        "m-001",
                        &["TRASH".into()],
                        &["INBOX".into()],
                    )
                    .await
                    .unwrap();
            }
            let run = h
                .authenticated()
                .start_sync(StartSyncRequest {
                    account_id: account.clone(),
                    full: false,
                    fetch_message_id: None,
                })
                .await
                .unwrap()
                .into_inner();
            sync_done(&h, run).await;
            let message = h
                .mail()
                .list_messages(ListMailRequest {
                    account_id: account.clone(),
                    page_size: 100,
                    ..Default::default()
                })
                .await
                .unwrap()
                .into_inner()
                .items
                .into_iter()
                .find(|m| m.provider_id == "m-001")
                .unwrap();
            let change = match action {
                "read" => change_message_request::Action::Read(MailReadChange { read: true }),
                "star" => change_message_request::Action::Star(MailStarChange { starred: true }),
                "archive" => change_message_request::Action::Archive(MailArchiveChange {}),
                "trash" => change_message_request::Action::Trash(MailTrashChange { trashed: true }),
                "restore" => {
                    change_message_request::Action::Trash(MailTrashChange { trashed: false })
                }
                _ => change_message_request::Action::Label(MailLabelChange {
                    collection_id: message
                        .collections
                        .iter()
                        .find(|c| c.provider_id == "Label_project")
                        .unwrap()
                        .id
                        .clone(),
                    present: false,
                }),
            };
            h.arm("operation_before_dispatch").unwrap();
            let op = h
                .mail()
                .change_message(ChangeMessageRequest {
                    account_id: account.clone(),
                    message_id: message.id.clone(),
                    request_id: "d9d4b90f-a313-4e85-a476-e6d0c20a6a03".into(),
                    action: Some(change),
                })
                .await
                .unwrap()
                .into_inner();
            h.wait("operation_before_dispatch").await.unwrap();
            let bytes = backup(&h).await;
            let initial = h.google.control().snapshot().await;
            if already_applied {
                let endpoint = match action {
                    "trash" => "trash",
                    "restore" => "untrash",
                    _ => "modify",
                };
                h.google
                    .control()
                    .inject(Fault {
                        method: "POST".into(),
                        path: format!("/gmail/v1/users/me/messages/m-001/{endpoint}"),
                        account: Some("alpha@example.test".into()),
                        call: None,
                        phase: Phase::After,
                        action: FaultAction::Disconnect,
                    })
                    .await;
                h.release("operation_before_dispatch").unwrap();
                settled(&h, &account, &op.id, "applied").await;
            }
            restore(&mut h, &bytes, &account).await;
            let held = settled(&h, &account, &op.id, "uncertain").await;
            let before = h.google.control().snapshot().await;
            let (mut request, mut observed) = reconcile(
                &h,
                &held,
                ReconciliationMode::Observe,
                if already_applied {
                    "applied"
                } else {
                    "uncertain"
                },
            )
            .await;
            same_effects(&before, &h.google.control().snapshot().await);
            if !already_applied {
                assert_eq!(
                    observed.error_code.as_deref(),
                    Some("reconciliation_resume_required")
                );
                if action == "read" {
                    for number in 2..=10 {
                        eprintln!("explicit observation {number} before safe resume");
                        let (_, next) =
                            reconcile(&h, &observed, ReconciliationMode::Observe, "uncertain")
                                .await;
                        observed = next;
                        same_effects(&before, &h.google.control().snapshot().await);
                    }
                }
                (request, observed) =
                    reconcile(&h, &observed, ReconciliationMode::ResumeSafe, "applied").await;
            }
            let remote = h.google.control().snapshot().await;
            assert_eq!(
                writes(&remote),
                writes(&initial) + 1,
                "{action} duplicate write"
            );
            let labels = &remote.mail["alpha@example.test"].messages["m-001"].labels;
            assert!(match action {
                "read" => !labels.contains("UNREAD"),
                "star" => labels.contains("STARRED"),
                "archive" => !labels.contains("INBOX"),
                "trash" => labels.contains("TRASH"),
                "restore" => !labels.contains("TRASH"),
                _ => !labels.contains("Label_project"),
            });
            assert_eq!(
                serde_json::to_value(&remote.mail["beta@example.test"]).unwrap(),
                serde_json::to_value(&initial.mail["beta@example.test"]).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&remote.calendars).unwrap(),
                serde_json::to_value(&initial.calendars).unwrap()
            );
            assert!(remote.mail["alpha@example.test"].accepted_sends.is_empty());
            let local = h
                .mail()
                .get_message(MessageRequest {
                    account_id: account.clone(),
                    message_id: message.id,
                })
                .await
                .unwrap()
                .into_inner()
                .message
                .unwrap();
            assert_eq!(
                local
                    .collections
                    .iter()
                    .map(|c| c.provider_id.clone())
                    .collect::<std::collections::BTreeSet<_>>(),
                *labels
            );
            let attempts = h
                .operations()
                .list_attempts(ListOperationAttemptsRequest {
                    account_id: account.clone(),
                    operation_id: op.id.clone(),
                    page_size: 100,
                    page_token: None,
                })
                .await
                .unwrap()
                .into_inner()
                .items;
            let count = if already_applied {
                1
            } else if action == "read" {
                12
            } else {
                3
            };
            assert_eq!(attempts.len(), count);
            assert_eq!(
                attempts
                    .iter()
                    .map(|a| a.ordinal)
                    .collect::<std::collections::BTreeSet<_>>(),
                (1..=count as u32).collect()
            );
            assert_eq!(
                observed
                    .reconciliation
                    .as_ref()
                    .unwrap()
                    .first_attempt_ordinal,
                Some(if already_applied {
                    1
                } else if action == "read" {
                    11
                } else {
                    2
                })
            );

            replay_after_restart(&mut h, request, &observed).await;
            h.shutdown().await.unwrap();
        }
    }
}

#[tokio::test]
async fn restored_calendar_changes_separate_event_observation_from_notification_evidence() {
    for action in ["create", "update", "delete", "respond"] {
        for policy in [
            CalendarNotificationPolicy::None,
            CalendarNotificationPolicy::All,
        ] {
            for already_applied in [false, true] {
                let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
                let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
                    .await
                    .account_id
                    .unwrap();
                h.google.control().put_event("alpha@example.test","alpha@example.test",serde_json::json!({"id":"mutation01","summary":"Original","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"},"organizer":{"email":"organizer@example.test","self":false},"attendees":[{"email":"alpha@example.test","self":true,"responseStatus":"needsAction"},{"email":"other@example.test","responseStatus":"accepted"}],"provider_extension":{"keep":true}})).await.unwrap();
                let window = AgendaWindow {
                    from: "2026-10-01".into(),
                    to: "2026-10-05".into(),
                };
                let run = h
                    .calendar()
                    .refresh_agenda(RefreshAgendaRequest {
                        account_id: account.clone(),
                        window: Some(window.clone()),
                    })
                    .await
                    .unwrap()
                    .into_inner();
                sync_done(&h, run).await;
                let calendar = h
                    .calendar()
                    .list_calendars(ListCalendarsRequest {
                        account_id: account.clone(),
                        page_size: 100,
                        page_token: None,
                    })
                    .await
                    .unwrap()
                    .into_inner()
                    .items
                    .into_iter()
                    .find(|c| c.is_primary)
                    .unwrap();
                let event = h
                    .calendar()
                    .list_agenda(ListAgendaRequest {
                        account_id: account.clone(),
                        window: Some(window),
                        page_size: 100,
                        page_token: None,
                    })
                    .await
                    .unwrap()
                    .into_inner()
                    .items
                    .into_iter()
                    .find(|e| e.provider_id == "mutation01")
                    .unwrap();
                let (method, path, change) = match action {
                    "create" => (
                        "POST",
                        "/calendar/v3/calendars/alpha@example.test/events",
                        change_event_request::Action::Create(CalendarCreate {
                            event: Some(CalendarEventFields {
                                summary: Some("Reconciled creation".into()),
                                start: Some(EventTime {
                                    value: Some(event_time::Value::Date("2026-10-03".into())),
                                }),
                                end: Some(EventTime {
                                    value: Some(event_time::Value::Date("2026-10-04".into())),
                                }),
                                attendees: Some(CalendarAttendees {
                                    items: vec![CalendarAttendee {
                                        email: "recipient@example.test".into(),
                                        ..Default::default()
                                    }],
                                }),
                                ..Default::default()
                            }),
                        }),
                    ),
                    "update" => (
                        "PATCH",
                        "/calendar/v3/calendars/alpha@example.test/events/mutation01",
                        change_event_request::Action::Update(CalendarUpdate {
                            event_id: event.id.clone(),
                            expected_etag: event.etag.clone().unwrap(),
                            fields: Some(CalendarEventFields {
                                summary: Some("Edited once".into()),
                                ..Default::default()
                            }),
                            clear_fields: vec![],
                        }),
                    ),
                    "delete" => (
                        "DELETE",
                        "/calendar/v3/calendars/alpha@example.test/events/mutation01",
                        change_event_request::Action::Delete(CalendarDelete {
                            event_id: event.id.clone(),
                            expected_etag: event.etag.clone().unwrap(),
                        }),
                    ),
                    _ => (
                        "PATCH",
                        "/calendar/v3/calendars/alpha@example.test/events/mutation01",
                        change_event_request::Action::Respond(CalendarRespond {
                            event_id: event.id.clone(),
                            expected_etag: event.etag.clone().unwrap(),
                            response: CalendarResponseStatus::Accepted as i32,
                            comment: Some("Will attend".into()),
                        }),
                    ),
                };
                h.arm("operation_before_dispatch").unwrap();
                let op = h
                    .calendar()
                    .change_event(ChangeEventRequest {
                        account_id: account.clone(),
                        calendar_id: calendar.id.clone(),
                        request_id: "d9d4b90f-a313-4e85-a476-e6d0c20a6a03".into(),
                        scope: CalendarEditScope::Single as i32,
                        notification_policy: policy as i32,
                        action: Some(change),
                    })
                    .await
                    .unwrap()
                    .into_inner();
                h.wait("operation_before_dispatch").await.unwrap();
                let bytes = backup(&h).await;
                let initial = h.google.control().snapshot().await;
                if already_applied {
                    h.google
                        .control()
                        .inject(Fault {
                            method: method.into(),
                            path: path.into(),
                            account: Some("alpha@example.test".into()),
                            call: None,
                            phase: Phase::After,
                            action: FaultAction::Disconnect,
                        })
                        .await;
                    h.release("operation_before_dispatch").unwrap();
                    settled(
                        &h,
                        &account,
                        &op.id,
                        if policy == CalendarNotificationPolicy::None {
                            "applied"
                        } else {
                            "uncertain"
                        },
                    )
                    .await;
                }
                restore(&mut h, &bytes, &account).await;
                let held = settled(&h, &account, &op.id, "uncertain").await;
                let before = h.google.control().snapshot().await;
                let (mut request, mut observed) = reconcile(
                    &h,
                    &held,
                    ReconciliationMode::Observe,
                    if already_applied && policy == CalendarNotificationPolicy::None {
                        "applied"
                    } else {
                        "uncertain"
                    },
                )
                .await;
                same_effects(&before, &h.google.control().snapshot().await);
                if observed.state == "uncertain" {
                    assert_eq!(
                        observed.error_code.as_deref(),
                        Some(if already_applied {
                            "calendar_notifications_unconfirmed"
                        } else if action == "create" {
                            "restored_calendar_create_acceptance_unknown"
                        } else {
                            "reconciliation_resume_required"
                        })
                    );
                    let can_resume = !already_applied && action != "create";
                    (request, observed) = reconcile(
                        &h,
                        &observed,
                        ReconciliationMode::ResumeSafe,
                        if can_resume { "applied" } else { "uncertain" },
                    )
                    .await;
                    if !can_resume {
                        same_effects(&before, &h.google.control().snapshot().await);
                    }
                }
                let remote = h.google.control().snapshot().await;
                let changed = already_applied || action != "create";
                assert_eq!(
                    writes(&remote),
                    writes(&initial) + u64::from(changed),
                    "{action}, {policy:?}, applied before restore={already_applied}"
                );
                assert_eq!(
                    serde_json::to_value(&remote.mail).unwrap(),
                    serde_json::to_value(&initial.mail).unwrap()
                );
                assert_eq!(
                    serde_json::to_value(&remote.calendars["beta@example.test"]).unwrap(),
                    serde_json::to_value(&initial.calendars["beta@example.test"]).unwrap()
                );
                let calendar_remote = &remote.calendars["alpha@example.test"]["alpha@example.test"];
                let calendar_initial =
                    &initial.calendars["alpha@example.test"]["alpha@example.test"];
                assert_eq!(
                    calendar_remote.notifications.len(),
                    calendar_initial.notifications.len()
                        + usize::from(changed && policy == CalendarNotificationPolicy::All)
                );
                if action == "create" {
                    assert_eq!(
                        calendar_remote.events.len(),
                        calendar_initial.events.len() + usize::from(changed)
                    );
                } else {
                    let value = &calendar_remote.events["mutation01"];
                    match action {
                        "update" => {
                            assert_eq!(value["summary"], "Edited once");
                            assert_eq!(
                                value["provider_extension"],
                                calendar_initial.events["mutation01"]["provider_extension"]
                            );
                        }
                        "delete" => assert_eq!(value["status"], "cancelled"),
                        _ => {
                            let attendees = value["attendees"].as_array().unwrap();
                            assert_eq!(
                                attendees
                                    .iter()
                                    .find(|a| a["email"] == "alpha@example.test")
                                    .unwrap()["responseStatus"],
                                "accepted"
                            );
                            assert_eq!(
                                attendees
                                    .iter()
                                    .find(|a| a["email"] == "other@example.test")
                                    .unwrap()["responseStatus"],
                                "accepted"
                            );
                        }
                    }
                }
                let history = h
                    .operations()
                    .list_attempts(ListOperationAttemptsRequest {
                        account_id: account.clone(),
                        operation_id: op.id.clone(),
                        page_size: 100,
                        page_token: None,
                    })
                    .await
                    .unwrap()
                    .into_inner()
                    .items;
                assert_eq!(
                    history.iter().filter(|a| a.kind == "dispatch").count(),
                    usize::from(!already_applied && action != "create")
                );
                if already_applied {
                    assert!(history
                        .iter()
                        .flat_map(|a| &a.receipts)
                        .all(|r| r.source == "positive_read"));
                }
                replay_after_restart(&mut h, request, &observed).await;
                h.shutdown().await.unwrap();
            }
        }
    }
}
