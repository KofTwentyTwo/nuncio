//! Real inbound calendar synchronization routine.
//!
//! Mirrors `nunciod::sync`'s shape for the calendar engine: [`sync_with_backend`]
//! fetches events from a [`CalendarBackend`] for a single calendar collection and
//! time window, then persists each event via [`DatabaseEngine::save_calendar_event`].
//!
//! # Production entry point
//!
//! [`sync_caldav_account`] is the production entry point: it builds a real
//! [`nuncio_cal::CalDavClient`] from a persisted [`nuncio_core::AccountConfig`]
//! (its `collection_url`) plus the credential resolved from the OS keyring,
//! then delegates to [`sync_with_backend`]. [`sync_with_backend`] is the
//! shared fetch-and-persist seam that both the production client and the
//! test-only `MockCalendarBackend` flow through, mirroring `sync.rs`'s
//! `fetch_and_persist`.
use nuncio_cal::{CalDavAccountConfig, CalDavClient, CalendarBackend};
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
    tracing::debug!(
        calendar_id = %calendar_id,
        start_window,
        end_window,
        "calendar sync: fetching events from backend"
    );
    let events = backend
        .fetch_events(calendar_id, start_window, end_window)
        .await
        .inspect_err(|e| {
            tracing::warn!(
                calendar_id = %calendar_id,
                "calendar sync: backend fetch failed: {e}"
            );
        })?;
    let fetched = events.len();
    let mut synced = 0usize;
    for event in events {
        db.save_calendar_event(&event).await?;
        synced += 1;
    }
    tracing::info!(
        calendar_id = %calendar_id,
        fetched,
        synced,
        "calendar sync: persisted fetched events"
    );
    Ok(synced)
}

/// Production entry point: build a real [`CalDavClient`] for `account` from its
/// persisted `collection_url` and the `password` resolved from the OS keyring
/// vault, then fetch-and-persist `calendar_id`'s events in `[start_window,
/// end_window]` via [`sync_with_backend`]. Returns the number of events synced.
///
/// `account.collection_url` must be a fully-qualified CalDAV calendar
/// collection URL (validated at `AddAccount` time); the account's
/// `email_address` is used as the DAV basic-auth username. No fabricated data
/// is ever returned -- a transport or persistence failure surfaces as a
/// [`CalendarSyncError`].
pub async fn sync_caldav_account(
    db: &nuncio_store::db::DatabaseEngine,
    account: &nuncio_core::AccountConfig,
    password: &str,
    calendar_id: &str,
    start_window: i64,
    end_window: i64,
) -> Result<usize, CalendarSyncError> {
    tracing::info!(
        account_id = %account.id,
        calendar_id = %calendar_id,
        "calendar sync: dispatching CalDAV sync for account"
    );
    let client = CalDavClient::new(CalDavAccountConfig {
        account_id: account.id.clone(),
        caldav_url: account.dav_collection_url().unwrap_or_default().to_string(),
        username: account.email_address.clone(),
        auth_token: password.to_string(),
    });
    sync_with_backend(db, &client, calendar_id, start_window, end_window).await
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

    /// A calendar sync must emit a followable INFO event carrying the calendar
    /// id and the persisted count, so an operator can trace what a sync did.
    #[test]
    fn sync_with_backend_logs_calendar_id_and_synced_count() {
        use crate::test_tracing::with_recorder;
        use tracing::Level;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let (recorder, ()) = with_recorder(|| {
            runtime.block_on(async {
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
                sync_with_backend(&db, &mock, "cal-work", 0, i64::MAX)
                    .await
                    .expect("sync succeeds");
            });
        });

        let events = recorder.events();
        let persisted = events
            .iter()
            .find(|e| e.message() == "calendar sync: persisted fetched events")
            .expect("a domain event for the persisted sync must be logged");
        assert_eq!(persisted.level, Level::INFO);
        assert_eq!(
            persisted.fields.get("calendar_id").map(String::as_str),
            Some("cal-work")
        );
        assert_eq!(
            persisted.fields.get("synced").map(String::as_str),
            Some("1")
        );
    }
}
