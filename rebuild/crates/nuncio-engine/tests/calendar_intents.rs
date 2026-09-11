#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::calendar::{AgendaWindow, CalendarObject},
    store::{
        AttemptKind, AttemptOutcome, CalendarCatalogEntry, CalendarChangeReceipt,
        CalendarCheckpoint, ConnectedAccount, EnqueueCalendarChange, OperationReceipt, Store,
        StoreError,
    },
};
use serde_json::{json, Value};
use zeroize::Zeroizing;

async fn seed(store: &Store, account: &str) -> (String, String) {
    let credential_ref = format!("synthetic/{account}");
    store
        .prepare_credential(credential_ref.clone())
        .await
        .unwrap();
    store
        .connect_google(ConnectedAccount {
            id: account.into(),
            subject: account.into(),
            address: "alpha@example.test".into(),
            credential_ref,
        })
        .await
        .unwrap();
    let run = store.start_calendar_run(account.into(), 1).await.unwrap();
    store
        .begin_sync_run(account.into(), run.id.clone(), None)
        .await
        .unwrap();
    store
        .stage_calendar_entry(
            account.into(),
            run.id.clone(),
            CalendarCatalogEntry {
                provider_id: "alpha@example.test".into(),
                summary: None,
                time_zone: Some("America/Chicago".into()),
                access_role: "owner".into(),
                is_primary: true,
                provider_json: "{}".into(),
            },
        )
        .await
        .unwrap();
    store
        .promote_calendar_catalog(account.into(), run.id.clone())
        .await
        .unwrap();
    let calendar = store.calendars(account.into()).await.unwrap().remove(0).id;
    let event=CalendarObject::from_google(json!({"id":"event01","etag":"\"v1\"","summary":"Before","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"},"unknown":{"retain":true}}),"America/Chicago").unwrap();
    for expanded in [false, true] {
        store
            .stage_calendar_event(
                account.into(),
                run.id.clone(),
                calendar.clone(),
                expanded,
                event.clone(),
            )
            .await
            .unwrap();
    }
    store
        .promote_calendar_objects(
            account.into(),
            run.id.clone(),
            calendar.clone(),
            CalendarCheckpoint {
                cursor: "original-cursor".into(),
                full: true,
                time_zone: "America/Chicago".into(),
            },
            2,
        )
        .await
        .unwrap();
    let revision = store.calendars(account.into()).await.unwrap()[0].canonical_revision;
    store
        .promote_calendar_occurrences(
            account.into(),
            run.id.clone(),
            calendar.clone(),
            AgendaWindow::new("2026-10-01", "2026-10-03").unwrap(),
            revision,
            3,
        )
        .await
        .unwrap();
    store
        .finish_calendar_run(account.into(), run.id, 4)
        .await
        .unwrap();
    let local = store
        .calendar_event_by_provider(account.into(), calendar.clone(), "event01".into())
        .await
        .unwrap()
        .id;
    (calendar, local)
}
fn request(
    account: &str,
    calendar: &str,
    event: &str,
    notifications: &str,
) -> EnqueueCalendarChange {
    EnqueueCalendarChange {
        account_id: account.into(),
        calendar_id: calendar.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        action: json!({"action":"update","event_id":event,"expected_etag":"\"v1\"","scope":"single","notifications":notifications,"patch":{"summary":"After"}}),
    }
}
#[tokio::test]
async fn calendar_intent_receipt_and_notification_evidence_are_atomic_and_durable() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x47; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let (calendar, event) = seed(&store, &account).await;
    seed(&store, &other).await;
    let input = request(&account, &calendar, &event, "all");
    let before = store.status().await.unwrap().revision;
    let op = store
        .enqueue_calendar_change(input.clone(), 100)
        .await
        .unwrap();
    assert_eq!(op.kind, "calendar_change");
    assert_eq!(op.state, "queued");
    assert_eq!(store.status().await.unwrap().revision, before + 1);
    assert_eq!(
        store
            .enqueue_calendar_change(input.clone(), 100)
            .await
            .unwrap()
            .id,
        op.id
    );
    let mut different = input.clone();
    different.action["notifications"] = json!("none");
    assert!(matches!(
        store.enqueue_calendar_change(different, 100).await,
        Err(StoreError::VersionConflict)
    ));
    let mut cross = input.clone();
    cross.account_id = other;
    assert!(matches!(
        store.enqueue_calendar_change(cross, 100).await,
        Err(StoreError::NotFound)
    ));
    assert_eq!(
        store
            .calendar_event(account.clone(), calendar.clone(), event.clone())
            .await
            .unwrap()
            .event
            .object
            .summary
            .as_deref(),
        Some("Before")
    );
    let payload = store
        .calendar_change_payload(account.clone(), op.id.clone())
        .await
        .unwrap();
    assert_eq!(payload.provider_calendar_id, "alpha@example.test");
    assert_eq!(payload.provider_event_id, "event01");
    assert_eq!(payload.patch, json!({"summary":"After"}));
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 101)
        .await
        .unwrap();
    let before = store.status().await.unwrap().revision;
    let bare = AttemptOutcome::Applied(OperationReceipt {
        kind: "calendar_change".into(),
        source: "acknowledgement".into(),
        provider_id: Some("event01".into()),
        etag: Some("\"v2\"".into()),
    });
    assert!(store
        .finish_operation_attempt(account.clone(), op.id.clone(), 1, bare, 102)
        .await
        .is_err());
    let mut observed: Value = serde_json::from_str(
        &store
            .calendar_event(account.clone(), calendar.clone(), event.clone())
            .await
            .unwrap()
            .event
            .object
            .provider_json,
    )
    .unwrap();
    observed["etag"] = json!("\"v2\"");
    let wrong = AttemptOutcome::AppliedCalendar {
        source: "acknowledgement".into(),
        receipt: CalendarChangeReceipt::Event(observed.clone()),
    };
    assert!(store
        .finish_operation_attempt(account.clone(), op.id.clone(), 1, wrong, 102)
        .await
        .is_err());
    assert_eq!(store.status().await.unwrap().revision, before);
    store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            1,
            AttemptOutcome::Uncertain {
                code: "response_lost".into(),
            },
            103,
        )
        .await
        .unwrap();
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(
        store
            .enqueue_calendar_change(input.clone(), 104)
            .await
            .unwrap()
            .id,
        op.id
    );
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 104)
        .await
        .unwrap();
    observed["summary"] = json!("After");
    let before = store.status().await.unwrap().revision;
    let reconciled = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::AppliedCalendar {
                source: "positive_read".into(),
                receipt: CalendarChangeReceipt::Event(observed.clone()),
            },
            105,
        )
        .await
        .unwrap();
    assert_eq!(
        reconciled.state, "uncertain",
        "event content cannot prove requested notification effects"
    );
    assert!(!reconciled.needs_reconciliation);
    assert_eq!(
        reconciled.error_code.as_deref(),
        Some("calendar_notifications_unconfirmed")
    );
    assert_eq!(store.status().await.unwrap().revision, before + 2);
    let local = store
        .calendar_event(account.clone(), calendar.clone(), event)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&local.event.object.provider_json).unwrap(),
        observed
    );
    assert_eq!(
        store
            .calendar_coverage(
                account.clone(),
                calendar.clone(),
                AgendaWindow::new("2026-10-01", "2026-10-03").unwrap()
            )
            .await
            .unwrap()
            .state,
        "stale"
    );
    assert_eq!(
        store
            .calendar_cursor(account.clone(), calendar.clone())
            .await
            .unwrap()
            .as_deref(),
        Some("original-cursor")
    );
    let receipts = store
        .operation_attempts(account.clone(), op.id)
        .await
        .unwrap();
    assert_eq!(receipts[1].receipts[0].source, "positive_read");
    // Stable create identity is owned by the stored request, including across retries.
    let create = EnqueueCalendarChange {
        account_id: account.clone(),
        calendar_id: calendar.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        action: json!({"action":"create","scope":"single","notifications":"none","event":{"summary":"New","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"}}}),
    };
    let queued = store
        .enqueue_calendar_change(create.clone(), 106)
        .await
        .unwrap();
    let body = store
        .calendar_change_payload(account.clone(), queued.id.clone())
        .await
        .unwrap();
    assert_eq!(body.provider_event_id, body.patch["id"]);
    assert!(body.provider_event_id.len() >= 5);
    assert!(body
        .provider_event_id
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'v').contains(&b)));
    assert_eq!(
        store.enqueue_calendar_change(create, 107).await.unwrap().id,
        queued.id
    );
    store
        .begin_operation_attempt(
            account.clone(),
            queued.id.clone(),
            AttemptKind::Dispatch,
            107,
        )
        .await
        .unwrap();
    let mut created = body.patch;
    created["etag"] = json!("\"create-v1\"");
    assert_eq!(
        store
            .finish_operation_attempt(
                account.clone(),
                queued.id,
                1,
                AttemptOutcome::AppliedCalendar {
                    source: "acknowledgement".into(),
                    receipt: CalendarChangeReceipt::Event(created)
                },
                108
            )
            .await
            .unwrap()
            .state,
        "applied"
    );
    assert!(store
        .calendar_event_by_provider(account, calendar, body.provider_event_id)
        .await
        .is_ok());
    store.close().await.unwrap();
}

#[tokio::test]
async fn calendar_receipts_require_new_versions_and_delete_survives_source_disappearance() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x45; 32]))
        .await
        .unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let (calendar, event) = seed(&store, &account).await;
    let first = request(&account, &calendar, &event, "none");
    let queued = store
        .enqueue_calendar_change(first.clone(), 100)
        .await
        .unwrap();
    let delete = EnqueueCalendarChange {
        request_id: uuid::Uuid::new_v4().to_string(),
        action: json!({"action":"delete","event_id":event,"expected_etag":"\"v2\"","scope":"single","notifications":"none"}),
        ..first.clone()
    };
    let later = store
        .enqueue_calendar_change(delete.clone(), 100)
        .await
        .unwrap();
    assert!(
        matches!(
            store
                .begin_operation_attempt(
                    account.clone(),
                    later.id.clone(),
                    AttemptKind::Dispatch,
                    101
                )
                .await,
            Err(StoreError::VersionConflict)
        ),
        "later intent may not overtake earlier changes for that provider identity"
    );
    let payload = store
        .calendar_change_payload(account.clone(), queued.id.clone())
        .await
        .unwrap();
    let mut observed = payload.base.unwrap();
    observed["summary"] = json!("After");
    store
        .begin_operation_attempt(
            account.clone(),
            queued.id.clone(),
            AttemptKind::Dispatch,
            101,
        )
        .await
        .unwrap();
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                queued.id.clone(),
                1,
                AttemptOutcome::AppliedCalendar {
                    source: "acknowledgement".into(),
                    receipt: CalendarChangeReceipt::Event(observed.clone())
                },
                102
            )
            .await
            .is_err(),
        "matching content with the original ETag is not an acknowledgement of this change"
    );
    observed["etag"] = json!("\"v2\"");
    assert!(
        store
            .finish_operation_attempt(
                account.clone(),
                queued.id.clone(),
                1,
                AttemptOutcome::AppliedCalendar {
                    source: "positive_read".into(),
                    receipt: CalendarChangeReceipt::Event(observed.clone())
                },
                102
            )
            .await
            .is_err(),
        "receipt source must match the journalled attempt"
    );
    store
        .finish_operation_attempt(
            account.clone(),
            queued.id.clone(),
            1,
            AttemptOutcome::Uncertain {
                code: "response_lost".into(),
            },
            102,
        )
        .await
        .unwrap();
    store
        .begin_operation_attempt(
            account.clone(),
            queued.id.clone(),
            AttemptKind::Reconcile,
            103,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .finish_operation_attempt(
                account.clone(),
                queued.id,
                2,
                AttemptOutcome::AppliedCalendar {
                    source: "positive_read".into(),
                    receipt: CalendarChangeReceipt::Event(observed)
                },
                104
            )
            .await
            .unwrap()
            .state,
        "applied",
        "no requested notifications remain to reconcile"
    );
    let run = store
        .start_calendar_run(account.clone(), 105)
        .await
        .unwrap();
    store
        .begin_sync_run(account.clone(), run.id.clone(), None)
        .await
        .unwrap();
    store
        .promote_calendar_objects(
            account.clone(),
            run.id.clone(),
            calendar.clone(),
            CalendarCheckpoint {
                cursor: "new-cursor".into(),
                full: true,
                time_zone: "America/Chicago".into(),
            },
            106,
        )
        .await
        .unwrap();
    store
        .clear_calendar_staging(account.clone(), run.id.clone(), calendar.clone(), true)
        .await
        .unwrap();
    let revision = store.calendars(account.clone()).await.unwrap()[0].canonical_revision;
    store
        .promote_calendar_occurrences(
            account.clone(),
            run.id.clone(),
            calendar.clone(),
            AgendaWindow::new("2026-10-01", "2026-10-03").unwrap(),
            revision,
            106,
        )
        .await
        .unwrap();
    store
        .finish_calendar_run(account.clone(), run.id, 106)
        .await
        .unwrap();
    assert!(store
        .calendar_event(account.clone(), calendar.clone(), event.clone())
        .await
        .is_err());
    assert_eq!(
        store.enqueue_calendar_change(delete, 107).await.unwrap().id,
        later.id
    );
    store
        .begin_operation_attempt(
            account.clone(),
            later.id.clone(),
            AttemptKind::Dispatch,
            107,
        )
        .await
        .unwrap();
    assert!(store
        .finish_operation_attempt(
            account.clone(),
            later.id.clone(),
            1,
            AttemptOutcome::AppliedCalendar {
                source: "acknowledgement".into(),
                receipt: CalendarChangeReceipt::Deleted {
                    provider_event_id: "wrong".into()
                }
            },
            108
        )
        .await
        .is_err());
    assert_eq!(
        store
            .finish_operation_attempt(
                account.clone(),
                later.id,
                1,
                AttemptOutcome::AppliedCalendar {
                    source: "acknowledgement".into(),
                    receipt: CalendarChangeReceipt::Deleted {
                        provider_event_id: "event01".into()
                    }
                },
                108
            )
            .await
            .unwrap()
            .state,
        "applied"
    );
    let tombstone = store
        .calendar_event(account.clone(), calendar.clone(), event)
        .await
        .unwrap();
    assert_eq!(tombstone.event.object.status, "cancelled");
    assert_eq!(tombstone.event.object.etag, None);
    assert_eq!(
        store
            .calendar_cursor(account, calendar)
            .await
            .unwrap()
            .as_deref(),
        Some("new-cursor")
    );
    store.close().await.unwrap();
}

#[test]
fn receipt_comparison_accepts_time_normalization_without_relaxing_timezone_or_date_bounds() {
    use nuncio_engine::store::{CalendarChangePayload, CalendarWriteKind};
    let payload = CalendarChangePayload {
        kind: CalendarWriteKind::Update,
        provider_calendar_id: "primary".into(),
        provider_event_id: "event01".into(),
        local_event_id: "local".into(),
        time_zone: "America/Chicago".into(),
        expected_etag: Some("\"v1\"".into()),
        notifications: "none".into(),
        base: None,
        patch: json!({"start":{"dateTime":"2026-10-01T10:00:00","timeZone":"America/Chicago"},"end":{"dateTime":"2026-10-01T11:00:00","timeZone":"America/Chicago"}}),
    };
    let mut event = json!({"id":"event01","etag":"\"v2\"","start":{"dateTime":"2026-10-01T15:00:00Z","timeZone":"America/Chicago"},"end":{"dateTime":"2026-10-01T16:00:00Z","timeZone":"America/Chicago"}});
    assert!(payload.satisfied_by(&CalendarChangeReceipt::Event(event.clone())));
    event["start"]["timeZone"] = json!("UTC");
    assert!(!payload.satisfied_by(&CalendarChangeReceipt::Event(event.clone())));
    event["start"]["timeZone"] = json!("America/Chicago");
    event["end"]["dateTime"] = json!("2026-10-01T17:00:00Z");
    assert!(!payload.satisfied_by(&CalendarChangeReceipt::Event(event)));
}

#[test]
fn receipt_time_comparison_preserves_submillisecond_precision() {
    use nuncio_engine::store::{CalendarChangePayload, CalendarWriteKind};
    let payload = CalendarChangePayload {
        kind: CalendarWriteKind::Update,
        provider_calendar_id: "primary".into(),
        provider_event_id: "event01".into(),
        local_event_id: "local".into(),
        time_zone: "UTC".into(),
        expected_etag: Some("\"v1\"".into()),
        notifications: "none".into(),
        base: None,
        patch: json!({"start":{"dateTime":"2026-10-01T10:00:00.123456Z"}}),
    };
    assert!(!payload.satisfied_by(&CalendarChangeReceipt::Event(
        json!({"id":"event01","etag":"\"v2\"","start":{"dateTime":"2026-10-01T10:00:00.123999Z"}})
    )));
}

#[tokio::test]
async fn calendar_retry_requires_positive_unchanged_version_or_stable_create_absence() {
    use nuncio_engine::store::CalendarRetryEvidence;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x42; 32]))
        .await
        .unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let (calendar, event) = seed(&store, &account).await;
    let op = store
        .enqueue_calendar_change(request(&account, &calendar, &event, "all"), 100)
        .await
        .unwrap();
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 101)
        .await
        .unwrap();
    assert!(store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            1,
            AttemptOutcome::Repeatable {
                code: "unsafe_repeat".into(),
                retry_at_ms: 110
            },
            102
        )
        .await
        .is_err());
    store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            1,
            AttemptOutcome::Uncertain {
                code: "response_lost".into(),
            },
            102,
        )
        .await
        .unwrap();
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 103)
        .await
        .unwrap();
    let body = store
        .calendar_change_payload(account.clone(), op.id.clone())
        .await
        .unwrap()
        .base
        .unwrap();
    let mut changed = body.clone();
    changed["etag"] = json!("\"different\"");
    for proof in [
        CalendarRetryEvidence::Unchanged(changed),
        CalendarRetryEvidence::Missing {
            provider_event_id: "event01".into(),
        },
    ] {
        assert!(store
            .finish_operation_attempt(
                account.clone(),
                op.id.clone(),
                2,
                AttemptOutcome::RepeatableCalendar {
                    code: "not_observed".into(),
                    retry_at_ms: 110,
                    evidence: proof
                },
                104
            )
            .await
            .is_err());
    }
    let retry = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            2,
            AttemptOutcome::RepeatableCalendar {
                code: "unchanged_remote_version".into(),
                retry_at_ms: 110,
                evidence: CalendarRetryEvidence::Unchanged(body),
            },
            104,
        )
        .await
        .unwrap();
    assert_eq!(retry.state, "retry_wait");
    assert_eq!(retry.next_attempt_at_ms, Some(110));
    assert!(store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 109)
        .await
        .is_err());
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Dispatch, 110)
        .await
        .unwrap();
    store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            3,
            AttemptOutcome::Uncertain {
                code: "response_lost".into(),
            },
            111,
        )
        .await
        .unwrap();
    store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 112)
        .await
        .unwrap();
    let deferred = store
        .finish_operation_attempt(
            account.clone(),
            op.id.clone(),
            4,
            AttemptOutcome::ReconcileLater {
                code: "rate_limited".into(),
                retry_at_ms: 200,
            },
            113,
        )
        .await
        .unwrap();
    assert_eq!(deferred.state, "uncertain");
    assert!(deferred.needs_reconciliation);
    assert_eq!(deferred.next_attempt_at_ms, Some(200));
    assert!(store
        .begin_operation_attempt(account.clone(), op.id.clone(), AttemptKind::Reconcile, 199)
        .await
        .is_err());
    store
        .begin_operation_attempt(account.clone(), op.id, AttemptKind::Reconcile, 200)
        .await
        .unwrap();
    store.close().await.unwrap();
}

#[test]
fn create_retry_uses_the_same_provider_identity_and_requires_observed_absence() {
    use nuncio_engine::store::{CalendarChangePayload, CalendarRetryEvidence, CalendarWriteKind};
    let payload = CalendarChangePayload {
        kind: CalendarWriteKind::Create,
        provider_calendar_id: "primary".into(),
        provider_event_id: "create001".into(),
        local_event_id: "local".into(),
        time_zone: "UTC".into(),
        expected_etag: None,
        notifications: "all".into(),
        base: None,
        patch: json!({"id":"create001"}),
    };
    assert!(payload.permits_retry(&CalendarRetryEvidence::Missing {
        provider_event_id: "create001".into()
    }));
    assert!(!payload.permits_retry(&CalendarRetryEvidence::Missing {
        provider_event_id: "other001".into()
    }));
    assert!(!payload.permits_retry(&CalendarRetryEvidence::Unchanged(
        json!({"id":"create001","etag":"\"v1\""})
    )));
}
