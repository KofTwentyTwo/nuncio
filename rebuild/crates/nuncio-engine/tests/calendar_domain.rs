#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::calendar::{AgendaWindow, CalendarObject, EventTime};
#[test]
fn event_times_preserve_dates_offsets_exceptions_and_unknown_fields() {
    let fixture: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../nuncio-test-support/fixtures/calendar/canonical.json"
    ))
    .unwrap();
    let events: Vec<_> = fixture
        .into_iter()
        .map(|v| CalendarObject::from_google(v, "America/Chicago").unwrap())
        .collect();
    assert!(matches!(&events[0].start,Some(EventTime::Date{date}) if date=="2026-03-09"));
    assert!(matches!(&events[0].end,Some(EventTime::Date{date}) if date=="2026-03-10"));
    assert_eq!(
        events[2].recurring_provider_id.as_deref(),
        Some("dstseries")
    );
    assert_eq!(events[2].start_ms, Some(1772983800000));
    assert_eq!(events[3].status, "cancelled");
    assert!(events[3].start.is_none());
    assert!(events[3].original_start.is_some());
    let raw: serde_json::Value = serde_json::from_str(&events[0].provider_json).unwrap();
    assert_eq!(
        raw["extendedProperties"]["private"]["canary"],
        "retain-provider-fields"
    );
}
#[test]
fn date_window_uses_calendar_midnights_across_dst_and_rejects_invalid_ranges() {
    let window = AgendaWindow::new("2026-10-31", "2026-11-02").unwrap();
    let (start, end) = window.provider_bounds("America/Chicago").unwrap();
    assert_eq!(start, "2026-10-31T00:00:00-05:00");
    assert_eq!(end, "2026-11-02T00:00:00-06:00");
    assert!(AgendaWindow::new("2026-02-30", "2026-03-03").is_err());
    assert!(AgendaWindow::new("2026-03-03", "2026-03-03").is_err());
    assert!(AgendaWindow::new("2026-3-1", "2026-03-03").is_err());
    assert!(CalendarObject::from_google(serde_json::json!({"id":"bad","status":"confirmed","start":{"date":"2026-03-01"},"end":{"dateTime":"2026-03-02T00:00:00Z"}}),"UTC").is_err());
    assert!(CalendarObject::from_google(serde_json::json!({"id":"bad","status":"confirmed","start":{"dateTime":"2026-03-01T09:00:00"},"end":{"dateTime":"2026-03-01T10:00:00"}}),"UTC").is_err());
}
