#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::calendar::{AgendaWindow, CalendarObject},
    store::{AccountRecord, CalendarCatalogEntry, CalendarCheckpoint, Store},
};
use zeroize::Zeroizing;
fn entry() -> CalendarCatalogEntry {
    CalendarCatalogEntry {
        provider_id: "remote-primary".into(),
        summary: Some("Primary".into()),
        time_zone: Some("America/Chicago".into()),
        access_role: "owner".into(),
        is_primary: true,
        provider_json: "{}".into(),
    }
}
#[tokio::test]
async fn calendar_staging_coverage_and_shared_identity_survive_projection_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x68; 32]))
        .await
        .unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    store
        .add_account(AccountRecord {
            id: account.clone(),
            provider: "google".into(),
            address: "calendar@example.test".into(),
        })
        .await
        .unwrap();
    let run = store.start_calendar_run(account.clone(), 1).await.unwrap();
    store
        .begin_sync_run(account.clone(), run.id.clone(), None)
        .await
        .unwrap();
    store
        .stage_calendar_entry(account.clone(), run.id.clone(), entry())
        .await
        .unwrap();
    assert!(store.calendars(account.clone()).await.unwrap().is_empty());
    store
        .promote_calendar_catalog(account.clone(), run.id.clone())
        .await
        .unwrap();
    let calendar = store.calendars(account.clone()).await.unwrap().remove(0);
    let event = CalendarObject::from_google(
        serde_json::from_str(include_str!(
            "../../nuncio-test-support/fixtures/calendar/all-day-2.json"
        ))
        .unwrap(),
        "America/Chicago",
    )
    .unwrap();
    store
        .stage_calendar_event(
            account.clone(),
            run.id.clone(),
            calendar.id.clone(),
            false,
            event.clone(),
        )
        .await
        .unwrap();
    assert!(store
        .calendar_event_by_provider(
            account.clone(),
            calendar.id.clone(),
            event.provider_id.clone()
        )
        .await
        .is_err());
    store
        .promote_calendar_objects(
            account.clone(),
            run.id.clone(),
            calendar.id.clone(),
            CalendarCheckpoint {
                cursor: "canonical-token".into(),
                full: true,
                time_zone: "America/Chicago".into(),
            },
            2,
        )
        .await
        .unwrap();
    let canonical = store
        .calendar_event_by_provider(
            account.clone(),
            calendar.id.clone(),
            event.provider_id.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .calendar_coverage(
                account.clone(),
                calendar.id.clone(),
                AgendaWindow::new("2026-10-31", "2026-11-02").unwrap()
            )
            .await
            .unwrap()
            .state,
        "unavailable"
    );
    store
        .stage_calendar_event(
            account.clone(),
            run.id.clone(),
            calendar.id.clone(),
            true,
            event.clone(),
        )
        .await
        .unwrap();
    let revision = store.calendars(account.clone()).await.unwrap()[0].canonical_revision;
    store
        .promote_calendar_occurrences(
            account.clone(),
            run.id.clone(),
            calendar.id.clone(),
            AgendaWindow::new("2026-10-30", "2026-11-03").unwrap(),
            revision,
            3,
        )
        .await
        .unwrap();
    let coverage = store
        .calendar_coverage(
            account.clone(),
            calendar.id.clone(),
            AgendaWindow::new("2026-10-31", "2026-11-02").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(coverage.state, "current");
    let agenda = store
        .calendar_agenda(
            account.clone(),
            AgendaWindow::new("2026-10-31", "2026-11-01").unwrap(),
            1,
            None,
        )
        .await
        .unwrap();
    assert_eq!(agenda.items.len(), 1);
    assert_eq!(agenda.items[0].id, canonical.id);
    assert_eq!(agenda.coverage[0].state, "current");
    assert!(store
        .calendar_agenda(
            account.clone(),
            AgendaWindow::new("2026-11-02", "2026-11-03").unwrap(),
            1,
            None
        )
        .await
        .unwrap()
        .items
        .is_empty());
    let get = store
        .calendar_event(account.clone(), calendar.id.clone(), canonical.id.clone())
        .await
        .unwrap();
    assert_eq!(get.event.object.provider_id, "alldate02");
    assert_eq!(
        store
            .calendar_event_by_provider(
                account.clone(),
                calendar.id.clone(),
                event.provider_id.clone()
            )
            .await
            .unwrap()
            .id,
        canonical.id
    );
    let mut changed = event;
    changed.summary = Some("Changed".into());
    store
        .stage_calendar_event(
            account.clone(),
            run.id.clone(),
            calendar.id.clone(),
            false,
            changed.clone(),
        )
        .await
        .unwrap();
    store
        .promote_calendar_objects(
            account.clone(),
            run.id.clone(),
            calendar.id.clone(),
            CalendarCheckpoint {
                cursor: "new-token".into(),
                full: false,
                time_zone: "America/Chicago".into(),
            },
            4,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .calendar_event_by_provider(
                account.clone(),
                calendar.id.clone(),
                changed.provider_id.clone()
            )
            .await
            .unwrap()
            .id,
        canonical.id
    );
    assert_eq!(
        store
            .calendar_coverage(
                account.clone(),
                calendar.id.clone(),
                AgendaWindow::new("2026-10-31", "2026-11-02").unwrap()
            )
            .await
            .unwrap()
            .state,
        "stale"
    );
    assert_eq!(
        store
            .calendar_cursor(account.clone(), calendar.id.clone())
            .await
            .unwrap()
            .as_deref(),
        Some("new-token")
    );
    let mut changed_zone = entry();
    changed_zone.time_zone = Some("UTC".into());
    store
        .stage_calendar_entry(account.clone(), run.id.clone(), changed_zone)
        .await
        .unwrap();
    store
        .promote_calendar_catalog(account.clone(), run.id.clone())
        .await
        .unwrap();
    assert!(store
        .calendar_cursor(account.clone(), calendar.id.clone())
        .await
        .unwrap()
        .is_none());
    let mut omitted_zone = entry();
    omitted_zone.time_zone = None;
    store
        .stage_calendar_entry(account.clone(), run.id.clone(), omitted_zone)
        .await
        .unwrap();
    store
        .promote_calendar_catalog(account.clone(), run.id.clone())
        .await
        .unwrap();
    let after = store.calendars(account.clone()).await.unwrap().remove(0);
    assert_eq!(after.id, calendar.id);
    assert_eq!(after.time_zone.as_deref(), Some("UTC"));
    assert_eq!(
        store
            .calendar_coverage(
                account,
                calendar.id,
                AgendaWindow::new("2026-10-31", "2026-11-02").unwrap()
            )
            .await
            .unwrap()
            .state,
        "stale"
    );
    store.close().await.unwrap();
}
