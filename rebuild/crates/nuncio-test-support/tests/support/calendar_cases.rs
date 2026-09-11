use super::{begin, finish, Seed, SystemHarness};
use nuncio_proto::v2::*;
use nuncio_test_support::google::{CursorScope, Fault, FaultAction, Phase};

fn window() -> AgendaWindow {
    AgendaWindow {
        from: "2026-03-01".into(),
        to: "2026-03-20".into(),
    }
}
fn query(account: &str) -> ListAgendaRequest {
    ListAgendaRequest {
        account_id: account.into(),
        window: Some(window()),
        page_size: 100,
        page_token: None,
    }
}
async fn refresh(h: &SystemHarness, account: &str) -> SyncRun {
    let mut run = h
        .calendar()
        .refresh_agenda(RefreshAgendaRequest {
            account_id: account.into(),
            window: Some(window()),
        })
        .await
        .unwrap()
        .into_inner();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(run.state.as_str(), "queued" | "running") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "Calendar run did not finish"
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
async fn catalog(h: &SystemHarness, account: &str) -> ListCalendarsResponse {
    h.calendar()
        .list_calendars(ListCalendarsRequest {
            account_id: account.into(),
            page_size: 100,
            page_token: None,
        })
        .await
        .unwrap()
        .into_inner()
}
fn date_time(time: &Option<EventTime>) -> &ZonedDateTime {
    let event_time::Value::DateTime(t) = time.as_ref().unwrap().value.as_ref().unwrap() else {
        unreachable!("Expected zoned date-time")
    };
    t
}
#[tokio::test]
async fn calendar_rpc_is_authenticated_scoped_paged_and_preserves_recurrence_and_optional_metadata()
{
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    h.google.control().set_page_overlap(true).await;
    h.google
        .control()
        .omit_calendar_fields("alpha@example.test", "primary", &["timeZone"])
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
    let mut anon = h.anonymous_calendar();
    assert_eq!(
        anon.list_calendars(ListCalendarsRequest {
            account_id: alpha.clone(),
            page_size: 100,
            page_token: None
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.list_agenda(query(&alpha)).await.unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.get_event(CalendarEventRequest {
            account_id: alpha.clone(),
            calendar_id: "unknown".into(),
            event_id: "unknown".into()
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anon.refresh_agenda(RefreshAgendaRequest {
            account_id: alpha.clone(),
            window: Some(window())
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::Unauthenticated
    );
    let run = refresh(&h, &alpha).await;
    assert_eq!(run.state, "succeeded", "{:?}", run.error_code);
    assert_eq!(refresh(&h, &beta).await.state, "succeeded");
    let calendars = catalog(&h, &alpha).await;
    assert_eq!(calendars.items.len(), 2);
    let first = h
        .calendar()
        .list_calendars(ListCalendarsRequest {
            account_id: alpha.clone(),
            page_size: 1,
            page_token: None,
        })
        .await
        .unwrap()
        .into_inner();
    let second = h
        .calendar()
        .list_calendars(ListCalendarsRequest {
            account_id: alpha.clone(),
            page_size: 1,
            page_token: first.next_page_token.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_ne!(first.items[0].id, second.items[0].id);
    assert!(second.next_page_token.is_none());
    assert_eq!(
        h.calendar()
            .list_calendars(ListCalendarsRequest {
                account_id: beta.clone(),
                page_size: 1,
                page_token: first.next_page_token
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(
        calendars
            .items
            .iter()
            .find(|c| c.is_primary)
            .unwrap()
            .time_zone
            .as_deref(),
        Some("America/Chicago")
    );
    let a = h
        .calendar()
        .list_agenda(query(&alpha))
        .await
        .unwrap()
        .into_inner();
    let b = h
        .calendar()
        .list_agenda(query(&beta))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(a.items.len(), 4);
    assert_eq!(b.items.len(), 4);
    assert!(a.coverage.iter().all(|c| c.state == "current"));
    for event in &a.items {
        let peer = b
            .items
            .iter()
            .find(|e| e.provider_id == event.provider_id)
            .unwrap();
        assert_ne!(peer.id, event.id);
        assert_eq!(
            h.calendar()
                .get_event(CalendarEventRequest {
                    account_id: beta.clone(),
                    calendar_id: event.calendar_id.clone(),
                    event_id: event.id.clone()
                })
                .await
                .unwrap_err()
                .code(),
            tonic::Code::NotFound
        );
    }
    let moved = a
        .items
        .iter()
        .find(|e| e.provider_id == "dstseries_20260308T140000Z")
        .unwrap();
    assert_eq!(date_time(&moved.start).rfc3339, "2026-03-08T10:30:00-05:00");
    assert_eq!(
        date_time(&moved.original_start).rfc3339,
        "2026-03-08T09:00:00-05:00"
    );
    assert_eq!(date_time(&moved.start).time_zone, "America/Chicago");
    assert_eq!(moved.recurring_provider_id.as_deref(), Some("dstseries"));
    let master = h
        .calendar()
        .get_event(CalendarEventRequest {
            account_id: alpha.clone(),
            calendar_id: moved.calendar_id.clone(),
            event_id: moved.recurring_event_id.clone().unwrap(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(master.coverage.unwrap().state, "current");
    let master = master.event.unwrap();
    assert_eq!(master.provider_id, "dstseries");
    let raw: serde_json::Value = serde_json::from_str(&master.provider_json).unwrap();
    assert_eq!(raw["recurrence"][0], "RRULE:FREQ=WEEKLY;COUNT=3");
    assert_eq!(raw["attendees"][0]["responseStatus"], "accepted");
    assert_eq!(raw["reminders"]["useDefault"], true);
    let cancelled = a.items.iter().find(|e| e.status == "cancelled").unwrap();
    assert!(cancelled.start.is_none());
    assert!(cancelled.original_start.is_some());
    let day = a
        .items
        .iter()
        .find(|e| e.provider_id == "alldate01")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&day.provider_json).unwrap()
            ["extendedProperties"]["private"]["canary"],
        "retain-provider-fields"
    );
    let mut q = query(&alpha);
    q.page_size = 1;
    let mut ids = Vec::new();
    loop {
        let p = h
            .calendar()
            .list_agenda(q.clone())
            .await
            .unwrap()
            .into_inner();
        ids.extend(p.items.iter().map(|e| e.id.clone()));
        if p.next_page_token.is_none() {
            break;
        }
        q.page_token = p.next_page_token;
    }
    assert_eq!(
        ids,
        a.items.iter().map(|e| e.id.clone()).collect::<Vec<_>>()
    );
    q.page_token = None;
    let first = h
        .calendar()
        .list_agenda(q.clone())
        .await
        .unwrap()
        .into_inner();
    q.page_token = first.next_page_token;
    let mut cross = q.clone();
    cross.account_id = beta.clone();
    assert_eq!(
        h.calendar().list_agenda(cross).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(refresh(&h, &alpha).await.state, "succeeded");
    assert_eq!(
        h.calendar().list_agenda(q).await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    let after = h
        .calendar()
        .list_agenda(query(&alpha))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        after.items.iter().map(|e| &e.id).collect::<Vec<_>>(),
        a.items.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    let mut outside = query(&alpha);
    outside.window = Some(AgendaWindow {
        from: "2028-01-01".into(),
        to: "2028-01-02".into(),
    });
    let outside = h
        .calendar()
        .list_agenda(outside)
        .await
        .unwrap()
        .into_inner();
    assert!(outside.items.is_empty());
    assert!(outside.coverage.iter().all(|c| c.state == "out_of_window"));
    let snapshot = h.google.control().snapshot().await;
    assert!(snapshot
        .calendars
        .values()
        .all(|calendars| calendars.values().all(|c| c.notifications.is_empty())));
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_canonical_delta_marks_old_occurrences_stale_until_expansion_and_retains_tombstones(
) {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let mut invalid = query(&account);
    invalid.window = Some(AgendaWindow {
        from: "2026-03-20".into(),
        to: "2026-03-01".into(),
    });
    assert_eq!(
        h.calendar().list_agenda(invalid).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(refresh(&h, &account).await.state, "succeeded");
    let before = h
        .calendar()
        .list_agenda(query(&account))
        .await
        .unwrap()
        .into_inner();
    let day = before
        .items
        .iter()
        .find(|e| e.provider_id == "alldate01")
        .unwrap();
    let mut remote: serde_json::Value = serde_json::from_str(&day.provider_json).unwrap();
    remote["summary"] = serde_json::json!("Externally changed all-day event");
    h.google
        .control()
        .put_event("alpha@example.test", "primary", remote)
        .await
        .unwrap();
    let path = "/calendar/v3/calendars/alpha@example.test/events";
    let count = h
        .google
        .control()
        .snapshot()
        .await
        .requests
        .iter()
        .find(|r| r.path == path && r.method == "GET")
        .unwrap()
        .count;
    // The one changed canonical resource completes before the expanded request fails.
    h.google
        .control()
        .inject(Fault {
            method: "GET".into(),
            path: path.into(),
            account: Some("alpha@example.test".into()),
            call: Some(count + 2),
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 503,
                retry_after_secs: None,
            },
        })
        .await;
    assert_eq!(refresh(&h, &account).await.state, "failed");
    let stale = h
        .calendar()
        .list_agenda(query(&account))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        stale.items.iter().find(|e| e.id == day.id).unwrap().summary,
        day.summary
    );
    assert_eq!(
        stale
            .coverage
            .iter()
            .find(|c| c.calendar_id == day.calendar_id)
            .unwrap()
            .state,
        "stale"
    );
    let request = CalendarEventRequest {
        account_id: account.clone(),
        calendar_id: day.calendar_id.clone(),
        event_id: day.id.clone(),
    };
    let canonical = h
        .calendar()
        .get_event(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        canonical.event.unwrap().summary.as_deref(),
        Some("Externally changed all-day event")
    );
    assert_eq!(canonical.coverage.unwrap().state, "stale");
    assert_eq!(refresh(&h, &account).await.state, "succeeded");
    let updated = h
        .calendar()
        .list_agenda(query(&account))
        .await
        .unwrap()
        .into_inner();
    assert!(updated.coverage.iter().all(|c| c.state == "current"));
    assert_eq!(
        updated
            .items
            .iter()
            .find(|e| e.id == day.id)
            .unwrap()
            .summary
            .as_deref(),
        Some("Externally changed all-day event")
    );
    h.google
        .control()
        .delete_event("alpha@example.test", "primary", "alldate01")
        .await
        .unwrap();
    assert_eq!(refresh(&h, &account).await.state, "succeeded");
    let tombstone = h
        .calendar()
        .get_event(request)
        .await
        .unwrap()
        .into_inner()
        .event
        .unwrap();
    assert_eq!(tombstone.status, "cancelled");
    assert!(tombstone.start.is_none());
    let after = h
        .calendar()
        .list_agenda(query(&account))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(after.items.len(), 3);
    assert!(after.items.iter().all(|e| e.id != day.id));
    let snapshot = h.google.control().snapshot().await;
    assert_eq!(
        snapshot.calendars["alpha@example.test"]["alpha@example.test"].events["alldate01"]
            ["status"],
        "cancelled"
    );
    assert!(
        snapshot.calendars["alpha@example.test"]["alpha@example.test"]
            .notifications
            .is_empty()
    );
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_expired_reset_and_discovery_failures_preserve_complete_local_state() {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    assert_eq!(refresh(&h, &account).await.state, "succeeded");
    let before = h
        .calendar()
        .list_agenda(query(&account))
        .await
        .unwrap()
        .into_inner();
    let primary = catalog(&h, &account)
        .await
        .items
        .into_iter()
        .find(|c| c.is_primary)
        .unwrap();
    h.google
        .control()
        .expire_cursor(
            "alpha@example.test",
            CursorScope::Calendar("alpha@example.test".into()),
        )
        .await
        .unwrap();
    let path = "/calendar/v3/calendars/alpha@example.test/events";
    let count = h
        .google
        .control()
        .snapshot()
        .await
        .requests
        .iter()
        .find(|r| r.method == "GET" && r.path == path)
        .map_or(0, |r| r.count);
    // Expired cursor, then the reset's first page, then fail its second page.
    assert!(count > 0, "Fault must target an observed provider route");
    h.google
        .control()
        .inject(Fault {
            method: "GET".into(),
            path: path.into(),
            account: Some("alpha@example.test".into()),
            call: Some(count + 3),
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 503,
                retry_after_secs: Some(1),
            },
        })
        .await;
    assert_eq!(refresh(&h, &account).await.state, "failed");
    let failed = h
        .calendar()
        .list_agenda(query(&account))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        failed
            .items
            .iter()
            .map(|e| (&e.id, &e.provider_json))
            .collect::<Vec<_>>(),
        before
            .items
            .iter()
            .map(|e| (&e.id, &e.provider_json))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        catalog(&h, &account)
            .await
            .items
            .into_iter()
            .find(|c| c.is_primary)
            .unwrap()
            .canonical_revision,
        primary.canonical_revision
    );
    assert_eq!(refresh(&h, &account).await.state, "succeeded");
    let recovered = h
        .calendar()
        .list_agenda(query(&account))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        recovered.items.iter().map(|e| &e.id).collect::<Vec<_>>(),
        before.items.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    h.google
        .control()
        .remove_calendar("alpha@example.test", "team-alpha@example.test")
        .await
        .unwrap();
    let path = "/calendar/v3/users/me/calendarList";
    let count = h
        .google
        .control()
        .snapshot()
        .await
        .requests
        .iter()
        .find(|r| r.method == "GET" && r.path == path)
        .map_or(0, |r| r.count);
    h.google
        .control()
        .inject(Fault {
            method: "GET".into(),
            path: path.into(),
            account: Some("alpha@example.test".into()),
            call: Some(count + 1),
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 503,
                retry_after_secs: None,
            },
        })
        .await;
    assert_eq!(refresh(&h, &account).await.state, "failed");
    assert!(catalog(&h, &account).await.items.iter().all(|c| !c.retired));
    assert_eq!(refresh(&h, &account).await.state, "succeeded");
    let after = catalog(&h, &account).await;
    assert_eq!(after.items.iter().filter(|c| c.retired).count(), 1);
    h.shutdown().await.unwrap();
    h.restart().await.unwrap();
    assert_eq!(
        catalog(&h, &account)
            .await
            .items
            .iter()
            .filter(|c| c.retired)
            .count(),
        1
    );
    h.shutdown().await.unwrap();
}
