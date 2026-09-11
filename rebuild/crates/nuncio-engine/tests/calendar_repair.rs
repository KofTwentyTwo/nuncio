#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::calendar::{AgendaWindow, CalendarObject},
    store::{
        AccountRecord, CalendarCatalogEntry, CalendarRepairCheckpoint, Store, StoreError,
        StoredCalendar,
    },
};
use serde_json::{json, Value};
use zeroize::Zeroizing;

#[tokio::test]
async fn catalog_permission_changes_and_retirement_invalidate_coverage_without_erasing_cached_events(
) {
    for repair in [false, true] {
        for role in ["freeBusyReader", "none"] {
            let temp = tempfile::tempdir().unwrap();
            let store = Store::open(temp.path(), Zeroizing::new(vec![0x51; 32]))
                .await
                .unwrap();
            let account = uuid::Uuid::new_v4().to_string();
            store
                .add_account(AccountRecord {
                    id: account.clone(),
                    provider: "google".into(),
                    address: "synthetic@example.test".into(),
                })
                .await
                .unwrap();
            let (run, calendars, checkpoints) =
                begin(&store, &account, &["secondary"], "cached").await;
            let calendar = &calendars[0].id;
            store
                .promote_calendar_repair(account.clone(), run, window(), checkpoints, 1001)
                .await
                .unwrap();
            let before = store
                .calendar_event_by_provider(account.clone(), calendar.clone(), "same-event".into())
                .await
                .unwrap();
            for (access_role, expected) in [(role, "unavailable"), ("reader", "stale")] {
                let run = store
                    .start_calendar_run(account.clone(), 1002)
                    .await
                    .unwrap();
                store
                    .begin_sync_run(account.clone(), run.id.clone(), None)
                    .await
                    .unwrap();
                let mut item = entry("secondary");
                item.access_role = access_role.into();
                store
                    .stage_calendar_entry(account.clone(), run.id.clone(), item)
                    .await
                    .unwrap();
                if repair && access_role == role {
                    store
                        .calendar_repair_targets(account.clone(), run.id.clone())
                        .await
                        .unwrap();
                    store
                        .promote_calendar_repair(account.clone(), run.id, window(), vec![], 1003)
                        .await
                        .unwrap();
                } else {
                    store
                        .promote_calendar_catalog(account.clone(), run.id.clone())
                        .await
                        .unwrap();
                    store
                        .finish_calendar_run(account.clone(), run.id, 1003)
                        .await
                        .unwrap();
                }
                let coverage = store
                    .calendar_coverage(account.clone(), calendar.clone(), window())
                    .await
                    .unwrap();
                assert_eq!(coverage.state, expected);
                assert!(store
                    .calendar_cursor(account.clone(), calendar.clone())
                    .await
                    .unwrap()
                    .is_none());
                let retained = store
                    .calendar_event_by_provider(
                        account.clone(),
                        calendar.clone(),
                        "same-event".into(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::to_value(retained).unwrap(),
                    serde_json::to_value(&before).unwrap()
                );
            }
            let (run, _, checkpoints) = begin(&store, &account, &["secondary"], "fresh").await;
            store
                .promote_calendar_repair(account.clone(), run, window(), checkpoints, 1004)
                .await
                .unwrap();
            assert_eq!(
                store
                    .calendar_coverage(account.clone(), calendar.clone(), window())
                    .await
                    .unwrap()
                    .state,
                "current"
            );
            let (run, _, checkpoints) = begin(&store, &account, &[], "empty").await;
            store
                .promote_calendar_repair(account.clone(), run, window(), checkpoints, 1005)
                .await
                .unwrap();
            assert_eq!(
                store
                    .calendar_coverage(account.clone(), calendar.clone(), window())
                    .await
                    .unwrap()
                    .state,
                "retired"
            );
            assert!(store
                .calendar_cursor(account.clone(), calendar.clone())
                .await
                .unwrap()
                .is_none());
            assert_eq!(
                store
                    .calendar_event_by_provider(
                        account.clone(),
                        calendar.clone(),
                        "same-event".into()
                    )
                    .await
                    .unwrap()
                    .id,
                before.id
            );
            store.close().await.unwrap();
        }
    }
}

fn window() -> AgendaWindow {
    AgendaWindow::new("2026-10-01", "2026-11-01").unwrap()
}
fn entry(id: &str) -> CalendarCatalogEntry {
    CalendarCatalogEntry {
        provider_id: id.into(),
        summary: Some(format!("Calendar {id}")),
        time_zone: Some("UTC".into()),
        access_role: "owner".into(),
        is_primary: id == "primary",
        provider_json: json!({"id":id}).to_string(),
    }
}
fn event(subject: &str) -> CalendarObject {
    CalendarObject::from_google(json!({"id":"same-event","status":"confirmed","etag":"\"repair-etag\"","summary":subject,"start":{"date":"2026-10-05"},"end":{"date":"2026-10-06"}}),"UTC").unwrap()
}
async fn begin(
    store: &Store,
    account: &str,
    names: &[&str],
    subject: &str,
) -> (String, Vec<StoredCalendar>, Vec<CalendarRepairCheckpoint>) {
    let run = store
        .start_calendar_run(account.into(), 1000)
        .await
        .unwrap();
    store
        .begin_sync_run(account.into(), run.id.clone(), None)
        .await
        .unwrap();
    for name in names {
        store
            .stage_calendar_entry(account.into(), run.id.clone(), entry(name))
            .await
            .unwrap();
    }
    let targets = store
        .calendar_repair_targets(account.into(), run.id.clone())
        .await
        .unwrap();
    let mut checkpoints = Vec::new();
    for calendar in &targets {
        for expanded in [false, true] {
            store
                .stage_calendar_repair_event(
                    account.into(),
                    run.id.clone(),
                    calendar.id.clone(),
                    expanded,
                    event(subject),
                )
                .await
                .unwrap();
        }
        checkpoints.push(CalendarRepairCheckpoint {
            calendar_id: calendar.id.clone(),
            cursor: format!("cursor-{subject}"),
            time_zone: "UTC".into(),
        });
    }
    (run.id, targets, checkpoints)
}
async fn published(store: &Store, account: &str) -> Value {
    serde_json::to_value(store.calendars(account.into()).await.unwrap()).unwrap()
}
#[tokio::test]
async fn calendar_repair_stages_new_identities_and_commits_all_calendars_only_with_complete_checkpoints(
) {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::open(temporary.path(), Zeroizing::new(vec![0x51; 32]))
        .await
        .unwrap();
    let alpha = uuid::Uuid::new_v4().to_string();
    let beta = uuid::Uuid::new_v4().to_string();
    for account in [&alpha, &beta] {
        store
            .add_account(AccountRecord {
                id: account.clone(),
                provider: "google".into(),
                address: format!("{account}@example.test"),
            })
            .await
            .unwrap();
        let (run, _, checkpoints) = begin(&store, account, &["primary"], "original").await;
        assert_eq!(published(&store, account).await, json!([]));
        store
            .promote_calendar_repair(account.clone(), run.clone(), window(), checkpoints, 1001)
            .await
            .unwrap();
        assert_eq!(
            store.sync_run(account.clone(), run).await.unwrap().state,
            "succeeded"
        );
    }
    let alpha_before = published(&store, &alpha).await;
    let beta_before = published(&store, &beta).await;
    let primary = alpha_before[0]["id"].as_str().unwrap();
    let old_event = store
        .calendar_event_by_provider(alpha.clone(), primary.into(), "same-event".into())
        .await
        .unwrap();
    let (run, targets, mut checkpoints) =
        begin(&store, &alpha, &["primary", "new-calendar"], "repaired").await;
    assert_eq!(published(&store, &alpha).await, alpha_before);
    assert_eq!(
        store
            .calendar_event_by_provider(alpha.clone(), primary.into(), "same-event".into())
            .await
            .unwrap()
            .object
            .provider_json,
        old_event.object.provider_json
    );
    assert_eq!(
        targets
            .iter()
            .find(|c| c.provider_id == "primary")
            .unwrap()
            .id,
        primary
    );
    let last = checkpoints.pop().unwrap();
    assert!(matches!(
        store
            .promote_calendar_repair(
                alpha.clone(),
                run.clone(),
                window(),
                checkpoints.clone(),
                1002
            )
            .await,
        Err(StoreError::InvalidInput)
    ));
    assert_eq!(published(&store, &alpha).await, alpha_before);
    checkpoints.push(last);
    let mut wrong = checkpoints.clone();
    wrong[0].calendar_id = beta_before[0]["id"].as_str().unwrap().into();
    assert!(matches!(
        store
            .promote_calendar_repair(alpha.clone(), run.clone(), window(), wrong, 1002)
            .await,
        Err(StoreError::InvalidInput)
    ));
    assert_eq!(published(&store, &beta).await, beta_before);
    let new_id = targets
        .iter()
        .find(|c| c.provider_id == "new-calendar")
        .unwrap()
        .id
        .clone();
    checkpoints.sort_by_key(|p| p.calendar_id == new_id);
    let connection = rusqlite::Connection::open(temporary.path().join("store.db")).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode([0x51; 32])))
        .unwrap();
    connection.execute_batch(&format!("CREATE TRIGGER reject_repair BEFORE INSERT ON calendar_occurrences WHEN NEW.calendar_id='{new_id}' BEGIN SELECT RAISE(ABORT,'synthetic repair storage failure'); END;")).unwrap();
    assert!(matches!(
        store
            .promote_calendar_repair(
                alpha.clone(),
                run.clone(),
                window(),
                checkpoints.clone(),
                1002
            )
            .await,
        Err(StoreError::Database(Some(1811)))
    ));
    assert_eq!(published(&store, &alpha).await, alpha_before);
    assert_eq!(
        store
            .calendar_event_by_provider(alpha.clone(), primary.into(), "same-event".into())
            .await
            .unwrap()
            .object
            .provider_json,
        old_event.object.provider_json
    );
    connection
        .execute_batch("DROP TRIGGER reject_repair")
        .unwrap();
    drop(connection);
    store
        .promote_calendar_repair(alpha.clone(), run.clone(), window(), checkpoints, 1002)
        .await
        .unwrap();
    assert_eq!(
        store.sync_run(alpha.clone(), run).await.unwrap().state,
        "succeeded"
    );
    assert_eq!(published(&store, &alpha).await.as_array().unwrap().len(), 2);
    for calendar in &targets {
        let object = store
            .calendar_event_by_provider(alpha.clone(), calendar.id.clone(), "same-event".into())
            .await
            .unwrap();
        assert_eq!(object.object.summary.as_deref(), Some("repaired"));
        if calendar.provider_id == "primary" {
            assert_eq!(object.id, old_event.id);
        }
        assert_eq!(
            store
                .calendar_coverage(alpha.clone(), calendar.id.clone(), window())
                .await
                .unwrap()
                .state,
            "current"
        );
    }
    assert_eq!(published(&store, &beta).await, beta_before);
    store.close().await.unwrap();
}
#[tokio::test]
async fn interrupted_calendar_repair_preserves_old_projection_and_rejects_late_publication_after_reopen(
) {
    let temporary = tempfile::tempdir().unwrap();
    let key = || Zeroizing::new(vec![0x61; 32]);
    let store = Store::open(temporary.path(), key()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    store
        .add_account(AccountRecord {
            id: account.clone(),
            provider: "google".into(),
            address: "alpha@example.test".into(),
        })
        .await
        .unwrap();
    let (run, _, checkpoints) = begin(&store, &account, &["primary"], "original").await;
    store
        .promote_calendar_repair(account.clone(), run, window(), checkpoints, 1001)
        .await
        .unwrap();
    let before = published(&store, &account).await;
    let (run, _, checkpoints) = begin(&store, &account, &["replacement"], "staged-only").await;
    store.close().await.unwrap();
    let store = Store::open(temporary.path(), key()).await.unwrap();
    store.recover_sync_runs(2000).await.unwrap();
    assert_eq!(published(&store, &account).await, before);
    assert_eq!(
        store
            .sync_run(account.clone(), run.clone())
            .await
            .unwrap()
            .error_code
            .as_deref(),
        Some("interrupted")
    );
    assert!(matches!(
        store
            .promote_calendar_repair(account.clone(), run, window(), checkpoints, 2001)
            .await,
        Err(StoreError::InvalidInput)
    ));
    assert_eq!(published(&store, &account).await, before);
    let (run, _, checkpoints) = begin(&store, &account, &["primary"], "recovered").await;
    store
        .promote_calendar_repair(account, run, window(), checkpoints, 2002)
        .await
        .unwrap();
    store.close().await.unwrap();
}
