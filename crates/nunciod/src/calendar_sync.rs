//! Real inbound calendar synchronization routine.
//!
//! Mirrors `nunciod::sync`'s shape for the calendar engine: [`sync_with_backend`]
//! fetches events from a [`CalendarBackend`] for a single calendar collection and
//! time window, then persists each event via [`DatabaseEngine::save_calendar_event`].
//!
//! # No production entry point yet
//!
//! Unlike `sync.rs`'s `run_account_sync`/`run_all_accounts_sync`, this module
//! deliberately has NO production entry point that resolves a real account's
//! CalDAV collection URL and credential and builds a real `CalDavClient` from
//! them: `nuncio_core::AccountConfig` carries no CalDAV fields today, so there
//! is nothing persisted to build one from. [`sync_with_backend`] is the only
//! function here; it is exercised directly in tests with `MockCalendarBackend`,
//! and it is the same seam a future story's production entry point will call
//! once per-account CalDAV configuration exists.
use nuncio_cal::CalendarBackend;
use nuncio_store::db::DatabaseError;
use thiserror::Error;

/// Errors that can occur while synchronizing a single calendar collection's events.
#[derive(Debug, Error)]
pub enum CalendarSyncError {
    /// The calendar backend (CalDAV/mock) reported an error.
    #[error("calendar backend error: {0}")]
    Backend(#[from] nuncio_cal::CalendarError),
    /// A database persistence error occurred while saving synced events.
    #[error("database error: {0}")]
    Store(#[from] DatabaseError),
}

/// Fetch every event in `calendar_id` whose window overlaps `[start_window,
/// end_window]` from `backend`, persisting each via
/// [`DatabaseEngine::save_calendar_event`]. Returns the number of events
/// processed.
///
/// Pure fetch-and-persist, unit-testable directly with a
/// [`nuncio_cal::MockCalendarBackend`] -- no event-bus or credential logic,
/// mirroring `nunciod::sync::fetch_and_persist`.
pub async fn sync_with_backend(
    db: &nuncio_store::db::DatabaseEngine,
    backend: &dyn CalendarBackend,
    calendar_id: &str,
    start_window: i64,
    end_window: i64,
) -> Result<usize, CalendarSyncError> {
    let events = backend
        .fetch_events(calendar_id, start_window, end_window)
        .await?;
    let mut synced = 0usize;
    for event in events {
        db.save_calendar_event(&event).await?;
        synced += 1;
    }
    Ok(synced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_cal::MockCalendarBackend;
    use nuncio_core::model::CalendarEvent;
    use nuncio_store::db::DatabaseEngine;

    fn mock_event(id: &str, calendar_id: &str, start: i64, end: i64) -> CalendarEvent {
        CalendarEvent {
            id: id.to_string(),
            account_id: "acct-cal-1".to_string(),
            calendar_id: calendar_id.to_string(),
            summary: format!("Event {id}"),
            start_time: start,
            end_time: end,
            rrule: None,
            location: None,
        }
    }

    #[tokio::test]
    async fn sync_with_backend_persists_mock_events() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");

        let mock = MockCalendarBackend::new();
        mock.add_event(mock_event(
            "evt-1",
            "cal-work",
            1_700_000_000,
            1_700_003_600,
        ));
        mock.add_event(mock_event(
            "evt-2",
            "cal-work",
            1_700_010_000,
            1_700_013_600,
        ));
        // Outside the requested window entirely -- must not be returned or persisted.
        mock.add_event(mock_event(
            "evt-out-of-window",
            "cal-work",
            1_800_000_000,
            1_800_003_600,
        ));

        let synced = sync_with_backend(&db, &mock, "cal-work", 1_699_999_000, 1_700_020_000)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 2);

        let persisted = db
            .list_calendar_events("acct-cal-1", 1_699_999_000, 1_700_020_000)
            .await
            .expect("list persisted events");
        assert_eq!(persisted.len(), 2);
        let ids: Vec<&str> = persisted.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"evt-1"));
        assert!(ids.contains(&"evt-2"));
    }

    #[tokio::test]
    async fn sync_with_backend_returns_zero_for_empty_backend() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let mock = MockCalendarBackend::new();

        let synced = sync_with_backend(&db, &mock, "cal-empty", 0, i64::MAX)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 0);
    }

    #[tokio::test]
    async fn sync_with_backend_surfaces_backend_failure() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let mock = MockCalendarBackend::new();
        mock.set_should_fail(true);

        let err = sync_with_backend(&db, &mock, "cal-work", 0, i64::MAX)
            .await
            .expect_err("sync fails");
        assert!(matches!(err, CalendarSyncError::Backend(_)));
        assert!(err.to_string().contains("calendar backend error"));
    }
}
