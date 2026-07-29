//! Protocol-agnostic async contacts backend trait definitions.

use async_trait::async_trait;

use crate::carddav::CardDavError;
use crate::models::Contact;

/// Protocol-agnostic contacts backend engine trait implemented by CardDAV and mock engines.
///
/// Mirrors `nuncio_cal::CalendarBackend`'s shape: `Send + Sync` so implementations can be held
/// behind an `Arc` and shared across daemon tasks, and returning the crate's single
/// [`CardDavError`] type so callers do not need to know whether a given implementation talks
/// to a real server or an in-memory fixture.
#[async_trait]
pub trait ContactsBackend: Send + Sync {
    /// Fetch contacts belonging to `account_id`.
    async fn fetch_contacts(&self, account_id: &str) -> Result<Vec<Contact>, CardDavError>;
}
