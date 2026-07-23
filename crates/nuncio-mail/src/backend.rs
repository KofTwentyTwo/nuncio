//! Protocol-agnostic async mail backend trait definitions.

use async_trait::async_trait;
use nuncio_core::model::{Email, Folder};

use crate::parser::MailError;

/// Protocol-agnostic mail backend engine trait implemented by JMAP and IMAP engines.
#[async_trait]
pub trait MailBackend: Send + Sync {
    /// Synchronize and list available mailbox folders.
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError>;

    /// Synchronize message envelopes for a specific folder since a checkpoint state.
    /// Returns the updated email list and the new server state checkpoint.
    async fn sync_messages(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<(Vec<Email>, String), MailError>;

    /// Send an email message over the configured transport.
    async fn send_email(&self, email: &Email) -> Result<(), MailError>;
}
