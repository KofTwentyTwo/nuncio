use super::{begin, finish, Seed, SystemHarness};
use nuncio_proto::v2::*;

#[tokio::test]
async fn limited_writer_api_preserves_private_events_and_applies_visible_edits() {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let provider_calendar = "team-alpha@example.test";
    h.google
        .control()
        .set_calendar_role(
            "alpha@example.test",
            provider_calendar,
            "writerWithoutPrivateAccess",
        )
        .await
        .unwrap();
    for (id, visibility, title) in [
        ("private001", "private", "Private API canary"),
        ("visible001", "default", "Visible"),
    ] {
        h.google.control().put_event("alpha@example.test", provider_calendar, serde_json::json!({"id":id,"visibility":visibility,"summary":title,"start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"},"attendees":[{"email":"recipient@example.test"}]})).await.unwrap();
    }
    let window = AgendaWindow {
        from: "2026-10-01".into(),
        to: "2026-10-05".into(),
    };
    let mut run = h
        .calendar()
        .refresh_agenda(RefreshAgendaRequest {
            account_id: account.clone(),
            window: Some(window.clone()),
        })
        .await
        .unwrap()
        .into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(run.state.as_str(), "queued" | "running") {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        run = h
            .authenticated()
            .get_sync_run(SyncRunRequest {
                account_id: account.clone(),
                run_id: run.id,
            })
            .await
            .unwrap()
            .into_inner();
    }
    assert_eq!(run.state, "succeeded");
    let events = h
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
        .items;
    let private = events
        .iter()
        .find(|e| e.provider_id == "private001")
        .unwrap();
    let visible = events
        .iter()
        .find(|e| e.provider_id == "visible001")
        .unwrap();
    assert!(private.summary.is_none());
    assert!(!private.provider_json.contains("canary"));
    let before = h.google.control().snapshot().await.calendars["alpha@example.test"]
        [provider_calendar]
        .clone();
    let request = ChangeEventRequest {
        account_id: account.clone(),
        calendar_id: visible.calendar_id.clone(),
        request_id: "d66afedf-37f4-4b61-901e-462852845f1e".into(),
        scope: CalendarEditScope::Single as i32,
        notification_policy: CalendarNotificationPolicy::All as i32,
        action: Some(change_event_request::Action::Update(CalendarUpdate {
            event_id: visible.id.clone(),
            expected_etag: visible.etag.clone().unwrap(),
            fields: Some(CalendarEventFields {
                summary: Some("API limited writer edited".into()),
                ..Default::default()
            }),
            clear_fields: vec![],
        })),
    };
    let applied = wait(
        &h,
        h.calendar()
            .change_event(request.clone())
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert_eq!(applied.state, "applied");
    assert_eq!(
        h.calendar()
            .change_event(request)
            .await
            .unwrap()
            .into_inner()
            .id,
        applied.id
    );
    let denied = ChangeEventRequest {
        account_id: account.clone(),
        calendar_id: private.calendar_id.clone(),
        request_id: "7c240d62-94bf-48cb-adc9-765b59475850".into(),
        scope: CalendarEditScope::Single as i32,
        notification_policy: CalendarNotificationPolicy::All as i32,
        action: Some(change_event_request::Action::Delete(CalendarDelete {
            event_id: private.id.clone(),
            expected_etag: private.etag.clone().unwrap(),
        })),
    };
    assert_eq!(
        h.calendar().change_event(denied).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let local = h
        .calendar()
        .get_event(CalendarEventRequest {
            account_id: account,
            calendar_id: visible.calendar_id.clone(),
            event_id: visible.id.clone(),
        })
        .await
        .unwrap()
        .into_inner()
        .event
        .unwrap();
    assert_eq!(local.summary.as_deref(), Some("API limited writer edited"));
    let after = h.google.control().snapshot().await.calendars["alpha@example.test"]
        [provider_calendar]
        .clone();
    assert_eq!(
        after.events["visible001"]["summary"],
        "API limited writer edited"
    );
    assert_eq!(after.events["private001"], before.events["private001"]);
    assert_eq!(after.version, before.version + 1);
    assert_eq!(after.notifications.len(), before.notifications.len() + 1);
    assert_eq!(
        after.notifications.last().unwrap().recipients,
        vec!["recipient@example.test"]
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_create_is_authenticated_scoped_and_records_one_independent_invitation_effect() {
    let (mut h, account, calendar) = setup().await;
    let request = ChangeEventRequest {
        account_id: account.clone(),
        calendar_id: calendar.id.clone(),
        request_id: "20be44ca-78e4-4f09-80c9-233b31458bdb".into(),
        scope: CalendarEditScope::Single as i32,
        notification_policy: CalendarNotificationPolicy::All as i32,
        action: Some(change_event_request::Action::Create(CalendarCreate {
            event: Some(CalendarEventFields {
                summary: Some("Offline API creation".into()),
                start: Some(EventTime {
                    value: Some(event_time::Value::Date("2026-10-02".into())),
                }),
                end: Some(EventTime {
                    value: Some(event_time::Value::Date("2026-10-03".into())),
                }),
                attendees: Some(CalendarAttendees {
                    items: vec![CalendarAttendee {
                        email: "recipient@example.test".into(),
                        ..Default::default()
                    }],
                }),
                ..Default::default()
            }),
        })),
    };
    assert_eq!(
        h.anonymous_calendar()
            .change_event(request.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let before = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    let mut op = h
        .calendar()
        .change_event(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        h.calendar()
            .change_event(request.clone())
            .await
            .unwrap()
            .into_inner()
            .id,
        op.id
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(op.state.as_str(), "queued" | "running" | "retry_wait")
        || op.needs_reconciliation
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "operation stayed {}",
            op.state
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        op = h
            .operations()
            .get_operation(OperationRequest {
                account_id: account.clone(),
                operation_id: op.id,
            })
            .await
            .unwrap()
            .into_inner();
    }
    assert_eq!(op.state, "applied");
    let local = h
        .calendar()
        .get_event(CalendarEventRequest {
            account_id: account.clone(),
            calendar_id: calendar.id.clone(),
            event_id: op.resource_id.clone(),
        })
        .await
        .unwrap()
        .into_inner()
        .event
        .unwrap();
    let after = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    assert_eq!(after.events.len(), before.events.len() + 1);
    assert_eq!(
        after.events[&local.provider_id]["summary"],
        "Offline API creation"
    );
    assert_eq!(after.notifications.len(), before.notifications.len() + 1);
    assert_eq!(
        after.events[&local.provider_id]["attendees"][0]["email"],
        "recipient@example.test"
    );
    let mut external = after.events[&local.provider_id].clone();
    external["summary"] = serde_json::json!("External version");
    h.google
        .control()
        .put_event("alpha@example.test", "alpha@example.test", external)
        .await
        .unwrap();
    let changed = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    let update = ChangeEventRequest {
        request_id: "6e224b9b-7e2a-4e5e-897f-bcbda85d5bb1".into(),
        action: Some(change_event_request::Action::Update(CalendarUpdate {
            event_id: local.id.clone(),
            expected_etag: local.etag.unwrap(),
            fields: Some(CalendarEventFields {
                summary: Some("Desired conflicting title".into()),
                ..Default::default()
            }),
            clear_fields: vec![],
        })),
        ..request
    };
    let conflict = wait(
        &h,
        h.calendar()
            .change_event(update)
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert_eq!(conflict.state, "conflict");
    assert_eq!(
        conflict.error_code.as_deref(),
        Some("calendar_precondition_failed")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&conflict.desired_state_json).unwrap()["patch"]
            ["summary"],
        "Desired conflicting title"
    );
    let observed = h
        .calendar()
        .get_event(CalendarEventRequest {
            account_id: account.clone(),
            calendar_id: calendar.id,
            event_id: local.id,
        })
        .await
        .unwrap()
        .into_inner()
        .event
        .unwrap();
    assert_eq!(observed.summary.as_deref(), Some("External version"));
    let history = h
        .operations()
        .list_attempts(ListOperationAttemptsRequest {
            account_id: account,
            operation_id: conflict.id,
            page_size: 100,
            page_token: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(history.items[0].receipts[0].kind, "calendar_conflict");
    assert_eq!(history.items[0].receipts[0].etag, observed.etag);
    let final_state = h.google.control().snapshot().await.calendars["alpha@example.test"]
        ["alpha@example.test"]
        .clone();
    assert_eq!(final_state.events, changed.events);
    assert_eq!(final_state.notifications.len(), changed.notifications.len());
    h.shutdown().await.unwrap();
}

async fn setup() -> (SystemHarness, String, CalendarSummary) {
    let h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    h.google.control().put_event("alpha@example.test","alpha@example.test",serde_json::json!({"id":"mutation01","summary":"Original","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"},"organizer":{"email":"organizer@example.test","self":false},"attendees":[{"email":"alpha@example.test","self":true,"responseStatus":"needsAction"},{"email":"other@example.test","responseStatus":"accepted"}],"provider_extension":{"keep":true}})).await.unwrap();
    let mut run = h
        .calendar()
        .refresh_agenda(RefreshAgendaRequest {
            account_id: account.clone(),
            window: Some(AgendaWindow {
                from: "2026-10-01".into(),
                to: "2026-10-05".into(),
            }),
        })
        .await
        .unwrap()
        .into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(run.state.as_str(), "queued" | "running") {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        run = h
            .authenticated()
            .get_sync_run(SyncRunRequest {
                account_id: account.clone(),
                run_id: run.id,
            })
            .await
            .unwrap()
            .into_inner();
    }
    assert_eq!(run.state, "succeeded");
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
    (h, account, calendar)
}

async fn wait(h: &SystemHarness, mut op: Operation) -> Operation {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
    while matches!(op.state.as_str(), "queued" | "running" | "retry_wait")
        || op.needs_reconciliation
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "operation stayed {} {:?}",
            op.state,
            op.error_code
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        op = h
            .operations()
            .get_operation(OperationRequest {
                account_id: op.account_id,
                operation_id: op.id,
            })
            .await
            .unwrap()
            .into_inner();
    }
    op
}

#[tokio::test]
async fn calendar_lost_acknowledgements_reconcile_event_state_without_repeating_notifications() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for action in ["create", "update", "delete", "respond"] {
        for policy in [
            CalendarNotificationPolicy::None,
            CalendarNotificationPolicy::All,
        ] {
            let (mut h, account, calendar) = setup().await;
            let events = h
                .calendar()
                .list_agenda(ListAgendaRequest {
                    account_id: account.clone(),
                    window: Some(AgendaWindow {
                        from: "2026-10-01".into(),
                        to: "2026-10-05".into(),
                    }),
                    page_size: 100,
                    page_token: None,
                })
                .await
                .unwrap()
                .into_inner();
            let event = events
                .items
                .into_iter()
                .find(|e| e.provider_id == "mutation01")
                .unwrap();
            let before = h.google.control().snapshot().await.calendars["alpha@example.test"]
                ["alpha@example.test"]
                .clone();
            let (method, path, change) = match action {
                "create" => (
                    "POST",
                    "/calendar/v3/calendars/alpha@example.test/events".to_string(),
                    change_event_request::Action::Create(CalendarCreate {
                        event: Some(CalendarEventFields {
                            summary: Some("Create after lost response".into()),
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
                    "/calendar/v3/calendars/alpha@example.test/events/mutation01".to_string(),
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
                    "/calendar/v3/calendars/alpha@example.test/events/mutation01".to_string(),
                    change_event_request::Action::Delete(CalendarDelete {
                        event_id: event.id.clone(),
                        expected_etag: event.etag.clone().unwrap(),
                    }),
                ),
                _ => (
                    "PATCH",
                    "/calendar/v3/calendars/alpha@example.test/events/mutation01".to_string(),
                    change_event_request::Action::Respond(CalendarRespond {
                        event_id: event.id.clone(),
                        expected_etag: event.etag.clone().unwrap(),
                        response: CalendarResponseStatus::Accepted as i32,
                        comment: Some("Will attend".into()),
                    }),
                ),
            };
            h.google
                .control()
                .inject(Fault {
                    method: method.into(),
                    path: path.clone(),
                    account: Some("alpha@example.test".into()),
                    call: None,
                    phase: Phase::After,
                    action: FaultAction::Disconnect,
                })
                .await;
            let request = ChangeEventRequest {
                account_id: account.clone(),
                calendar_id: calendar.id.clone(),
                request_id: "636e3905-f2e3-45d3-9c57-7274d0e29a50".into(),
                scope: CalendarEditScope::Single as i32,
                notification_policy: policy as i32,
                action: Some(change),
            };
            let op = wait(
                &h,
                h.calendar()
                    .change_event(request.clone())
                    .await
                    .unwrap()
                    .into_inner(),
            )
            .await;
            assert_eq!(
                op.state,
                if policy == CalendarNotificationPolicy::None {
                    "applied"
                } else {
                    "uncertain"
                }
            );
            assert!(!op.needs_reconciliation);
            if policy == CalendarNotificationPolicy::All {
                assert_eq!(
                    op.error_code.as_deref(),
                    Some("calendar_notifications_unconfirmed")
                );
            }
            assert_eq!(
                h.calendar()
                    .change_event(request)
                    .await
                    .unwrap()
                    .into_inner()
                    .id,
                op.id
            );
            let after = h.google.control().snapshot().await;
            assert_eq!(
                after
                    .requests
                    .iter()
                    .filter(|r| r.method == method && r.path == path)
                    .map(|r| r.count)
                    .sum::<u64>(),
                1,
                "lost acknowledgement must not produce a second write"
            );
            let remote = &after.calendars["alpha@example.test"]["alpha@example.test"];
            assert_eq!(remote.version, before.version + 1);
            assert_eq!(
                remote.notifications.len(),
                before.notifications.len() + usize::from(policy == CalendarNotificationPolicy::All)
            );
            let local = h
                .calendar()
                .get_event(CalendarEventRequest {
                    account_id: account.clone(),
                    calendar_id: calendar.id,
                    event_id: op.resource_id,
                })
                .await
                .unwrap()
                .into_inner()
                .event
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&local.provider_json).unwrap(),
                remote.events[&local.provider_id]
            );
            match action {
                "create" => assert_eq!(remote.events.len(), before.events.len() + 1),
                "update" => assert_eq!(remote.events["mutation01"]["summary"], "Edited once"),
                "delete" => assert_eq!(remote.events["mutation01"]["status"], "cancelled"),
                _ => {
                    assert_eq!(
                        remote.events["mutation01"]["attendees"][0]["responseStatus"],
                        "accepted"
                    );
                    assert_eq!(
                        remote.events["mutation01"]["attendees"][1],
                        before.events["mutation01"]["attendees"][1]
                    );
                }
            }
            if action != "create" {
                assert_eq!(
                    remote.events["mutation01"]["provider_extension"],
                    before.events["mutation01"]["provider_extension"]
                );
            }
            let history = h
                .operations()
                .list_attempts(ListOperationAttemptsRequest {
                    account_id: account,
                    operation_id: op.id,
                    page_size: 100,
                    page_token: None,
                })
                .await
                .unwrap()
                .into_inner();
            assert_eq!(history.items.len(), 2);
            assert_eq!(history.items[0].receipts[0].source, "positive_read");
            h.shutdown().await.unwrap();
        }
    }
}

#[tokio::test]
async fn freebusy_api_is_authenticated_and_reports_coverage_without_local_agenda_assumptions() {
    let (mut h, account, _) = setup().await;
    let q = FreeBusyRequest {
        account_id: account.clone(),
        from: "2026-10-02T00:00:00Z".into(),
        to: "2026-10-04T00:00:00Z".into(),
        time_zone: "America/Chicago".into(),
        provider_calendar_ids: vec![
            "primary".into(),
            "team-alpha@example.test".into(),
            "missing@example.test".into(),
        ],
    };
    assert_eq!(
        h.anonymous_calendar()
            .query_free_busy(q.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let before = h.google.control().snapshot().await;
    let result = h
        .calendar()
        .query_free_busy(q.clone())
        .await
        .unwrap()
        .into_inner();
    assert!(!result.complete);
    assert_eq!(result.from, q.from);
    assert_eq!(result.to, q.to);
    assert!(result.checked_at_ms > 0);
    assert_eq!(result.calendars.len(), 3);
    assert!(result.calendars[0].complete);
    assert_eq!(
        result.calendars[0].busy,
        vec![BusyPeriod {
            from: "2026-10-02T00:00:00-05:00".into(),
            to: "2026-10-03T00:00:00-05:00".into()
        }]
    );
    assert!(result.calendars[1].complete);
    assert!(result.calendars[1].busy.is_empty());
    assert!(!result.calendars[2].complete);
    assert_eq!(result.calendars[2].errors[0].reason, "notFound");
    let after = h.google.control().snapshot().await;
    let a = &after.calendars["alpha@example.test"]["alpha@example.test"];
    let b = &before.calendars["alpha@example.test"]["alpha@example.test"];
    assert_eq!(a.events, b.events);
    assert_eq!(a.version, b.version);
    assert_eq!(a.notifications.len(), b.notifications.len());
    for bad in [
        FreeBusyRequest {
            provider_calendar_ids: vec![],
            ..q.clone()
        },
        FreeBusyRequest {
            from: "bad".into(),
            ..q.clone()
        },
        FreeBusyRequest {
            time_zone: "Unknown".into(),
            ..q.clone()
        },
    ] {
        assert_eq!(
            h.calendar().query_free_busy(bad).await.unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
    }
    assert_eq!(
        h.calendar()
            .query_free_busy(FreeBusyRequest {
                account_id: "bd89c9fe-324d-476c-b326-cc4e9902ddcb".into(),
                ..q
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::NotFound
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_write_faults_retry_only_with_rejection_or_positive_identity_evidence() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for (phase, fault, expected, writes) in [
        (
            Phase::Before,
            FaultAction::Status {
                code: 401,
                retry_after_secs: None,
            },
            "applied",
            2,
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 403,
                retry_after_secs: None,
            },
            "failed",
            1,
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 429,
                retry_after_secs: Some(1),
            },
            "applied",
            2,
        ),
        (
            Phase::Before,
            FaultAction::Status {
                code: 503,
                retry_after_secs: Some(1),
            },
            "applied",
            2,
        ),
        (Phase::Before, FaultAction::Disconnect, "applied", 2),
        (
            Phase::After,
            FaultAction::Status {
                code: 503,
                retry_after_secs: Some(1),
            },
            "applied",
            1,
        ),
        (Phase::After, FaultAction::MalformedJson, "applied", 1),
        (Phase::After, FaultAction::TruncatedBody, "applied", 1),
    ] {
        let (mut h, account, calendar) = setup().await;
        let before = h.google.control().snapshot().await;
        let path = "/calendar/v3/calendars/alpha@example.test/events";
        h.google
            .control()
            .inject(Fault {
                method: "POST".into(),
                path: path.into(),
                account: Some("alpha@example.test".into()),
                call: None,
                phase,
                action: fault,
            })
            .await;
        let request = ChangeEventRequest {
            account_id: account.clone(),
            calendar_id: calendar.id,
            request_id: "b4a818fa-80ae-4b9b-9c8a-b3d7e6c16e75".into(),
            scope: CalendarEditScope::Single as i32,
            notification_policy: CalendarNotificationPolicy::None as i32,
            action: Some(change_event_request::Action::Create(CalendarCreate {
                event: Some(CalendarEventFields {
                    summary: Some("Fault-safe create".into()),
                    start: Some(EventTime {
                        value: Some(event_time::Value::Date("2026-10-03".into())),
                    }),
                    end: Some(EventTime {
                        value: Some(event_time::Value::Date("2026-10-04".into())),
                    }),
                    ..Default::default()
                }),
            })),
        };
        let op = wait(
            &h,
            h.calendar()
                .change_event(request)
                .await
                .unwrap()
                .into_inner(),
        )
        .await;
        assert_eq!(op.state, expected, "{:?}", op.error_code);
        let after = h.google.control().snapshot().await;
        let a = &after.calendars["alpha@example.test"]["alpha@example.test"];
        let b = &before.calendars["alpha@example.test"]["alpha@example.test"];
        assert_eq!(
            a.events.len(),
            b.events.len() + usize::from(expected == "applied")
        );
        assert_eq!(a.version, b.version + u64::from(expected == "applied"));
        assert_eq!(a.notifications.len(), b.notifications.len());
        assert_eq!(
            after
                .requests
                .iter()
                .filter(|r| r.path == path && r.method == "POST")
                .map(|r| r.count)
                .sum::<u64>(),
            writes
        );
        assert_eq!(
            h.accounts()
                .list_accounts(ListAccountsRequest {})
                .await
                .unwrap()
                .into_inner()
                .accounts[0]
                .state,
            "connected"
        );
        let history = h
            .operations()
            .list_attempts(ListOperationAttemptsRequest {
                account_id: account,
                operation_id: op.id,
                page_size: 100,
                page_token: None,
            })
            .await
            .unwrap()
            .into_inner();
        if phase == Phase::After {
            assert_eq!(history.items[0].receipts[0].source, "positive_read");
        }
        h.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn freebusy_faults_do_not_report_free_time_and_backoff_survives_engine_restart() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for (fault, code) in [
        (FaultAction::MalformedJson, Some(tonic::Code::Internal)),
        (FaultAction::TruncatedBody, Some(tonic::Code::Unavailable)),
        (FaultAction::Disconnect, Some(tonic::Code::Unavailable)),
        (
            FaultAction::Status {
                code: 503,
                retry_after_secs: None,
            },
            Some(tonic::Code::Unavailable),
        ),
        (
            FaultAction::Status {
                code: 401,
                retry_after_secs: None,
            },
            None,
        ),
        (
            FaultAction::Status {
                code: 403,
                retry_after_secs: None,
            },
            Some(tonic::Code::Unauthenticated),
        ),
    ] {
        let (mut h, account, _) = setup().await;
        let q = FreeBusyRequest {
            account_id: account,
            from: "2026-10-02T00:00:00Z".into(),
            to: "2026-10-04T00:00:00Z".into(),
            time_zone: "UTC".into(),
            provider_calendar_ids: vec!["primary".into()],
        };
        h.google
            .control()
            .inject(Fault {
                method: "POST".into(),
                path: "/calendar/v3/freeBusy".into(),
                account: Some("alpha@example.test".into()),
                call: None,
                phase: Phase::Before,
                action: fault,
            })
            .await;
        let response = h.calendar().query_free_busy(q).await;
        if let Some(code) = code {
            assert_eq!(response.unwrap_err().code(), code);
        } else {
            assert!(response.unwrap().into_inner().complete);
        }
        let snapshot = h.google.control().snapshot().await;
        assert_eq!(
            snapshot
                .requests
                .iter()
                .filter(|r| r.path == "/calendar/v3/freeBusy" && r.method == "POST")
                .map(|r| r.count)
                .sum::<u64>(),
            if code.is_none() { 2 } else { 1 }
        );
        h.shutdown().await.unwrap();
    }
    let (mut h, account, _) = setup().await;
    let q = FreeBusyRequest {
        account_id: account,
        from: "2026-10-02T00:00:00Z".into(),
        to: "2026-10-04T00:00:00Z".into(),
        time_zone: "UTC".into(),
        provider_calendar_ids: vec!["primary".into()],
    };
    h.google
        .control()
        .inject(Fault {
            method: "POST".into(),
            path: "/calendar/v3/freeBusy".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 429,
                retry_after_secs: Some(2),
            },
        })
        .await;
    let start = tokio::time::Instant::now();
    assert_eq!(
        h.calendar()
            .query_free_busy(q.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unavailable
    );
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(
        h.calendar()
            .query_free_busy(q.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unavailable
    );
    assert_eq!(
        h.google
            .control()
            .snapshot()
            .await
            .requests
            .iter()
            .filter(|r| r.path == "/calendar/v3/freeBusy" && r.method == "POST")
            .map(|r| r.count)
            .sum::<u64>(),
        1
    );
    tokio::time::sleep_until(start + std::time::Duration::from_millis(2100)).await;
    assert!(
        h.calendar()
            .query_free_busy(q)
            .await
            .unwrap()
            .into_inner()
            .complete
    );
    h.shutdown().await.unwrap();
}
