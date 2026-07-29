//! Protocol-agnostic async calendar backend trait definitions.

use async_trait::async_trait;
use nuncio_core::model::CalendarEvent;

use crate::parser::CalendarError;

/// Protocol-agnostic calendar backend engine trait implemented by CalDAV and mock engines.
///
/// Mirrors `nuncio_mail::MailBackend`'s shape: `Send + Sync` so implementations can be held
/// behind an `Arc` and shared across daemon tasks, and returning the crate's single
/// [`CalendarError`] type so callers do not need to know whether a given implementation talks
/// to a real server or an in-memory fixture.
#[async_trait]
pub trait CalendarBackend: Send + Sync {
    /// Fetch events belonging to `calendar_id` whose window overlaps
    /// `[start_window, end_window]` (inclusive, unix seconds).
    async fn fetch_events(
        &self,
        calendar_id: &str,
        start_window: i64,
        end_window: i64,
    ) -> Result<Vec<CalendarEvent>, CalendarError>;
}
