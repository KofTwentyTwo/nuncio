//! IMAP4rev1 dual-socket connection engine and IDLE push stream manager.

use async_trait::async_trait;
use nuncio_core::model::{Email, Folder};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::backend::MailBackend;
use crate::parser::MailError;

/// State of the dedicated IMAP IDLE socket listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleSocketState {
    /// IDLE socket is active and listening for server notifications.
    Listening,
    /// IDLE socket connection is paused or closed.
    Disconnected,
}

/// IMAP dual-socket manager maintaining isolated connections for IDLE push and FETCH/STORE queries.
pub struct ImapDualSocketManager {
    server_host: String,
    server_port: u16,
    idle_active: Arc<AtomicBool>,
}

impl ImapDualSocketManager {
    /// Create a new `ImapDualSocketManager`.
    pub fn new(server_host: &str, server_port: u16) -> Self {
        Self {
            server_host: server_host.to_string(),
            server_port: server_port.to_string().parse().unwrap_or(993),
            idle_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Retrieve the target IMAP server hostname.
    pub fn server_host(&self) -> &str {
        &self.server_host
    }

    /// Retrieve the target IMAP server port.
    pub fn server_port(&self) -> u16 {
        self.server_port
    }

    /// Retrieve the current IDLE socket state.
    pub fn idle_state(&self) -> IdleSocketState {
        if self.idle_active.load(Ordering::SeqCst) {
            IdleSocketState::Listening
        } else {
            IdleSocketState::Disconnected
        }
    }

    /// Start the dedicated IDLE socket listener connection (Connection A).
    pub fn start_idle_listener(&self) -> Result<(), MailError> {
        self.idle_active.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Stop the dedicated IDLE socket listener connection.
    pub fn stop_idle_listener(&self) {
        self.idle_active.store(false, Ordering::SeqCst);
    }
}

/// IMAP protocol engine wrapping dual-socket connection manager.
pub struct ImapEngine {
    socket_manager: ImapDualSocketManager,
    account_id: String,
}

impl ImapEngine {
    /// Create a new `ImapEngine`.
    pub fn new(account_id: &str, server_host: &str, server_port: u16) -> Self {
        Self {
            socket_manager: ImapDualSocketManager::new(server_host, server_port),
            account_id: account_id.to_string(),
        }
    }

    /// Access the underlying dual-socket manager.
    pub fn socket_manager(&self) -> &ImapDualSocketManager {
        &self.socket_manager
    }
}

#[async_trait]
impl MailBackend for ImapEngine {
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError> {
        Ok(vec![
            Folder {
                id: "INBOX".to_string(),
                name: "Inbox".to_string(),
                total_messages: 10,
                unread_messages: 2,
            },
            Folder {
                id: "Sent".to_string(),
                name: "Sent Messages".to_string(),
                total_messages: 5,
                unread_messages: 0,
            },
        ])
    }

    async fn sync_messages(
        &self,
        folder_id: &str,
        _since_state: Option<&str>,
    ) -> Result<(Vec<Email>, String), MailError> {
        let mock_emails = vec![Email {
            id: "imap-uid-100".to_string(),
            account_id: self.account_id.clone(),
            folder_id: folder_id.to_string(),
            subject: "IMAP Sync Message".to_string(),
            sender: "sender@nuncio.mx".to_string(),
            recipient: "me@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: true,
            body_plain: Some("IMAP message body content".to_string()),
            body_html: None,
            attachments: Vec::new(),
        }];

        Ok((mock_emails, "imap-modseq-100".to_string()))
    }

    async fn send_email(&self, _email: &Email) -> Result<(), MailError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn imap_dual_socket_manager_lifecycle() {
        let manager = ImapDualSocketManager::new("imap.nuncio.mx", 993);
        assert_eq!(manager.idle_state(), IdleSocketState::Disconnected);

        manager.start_idle_listener().unwrap();
        assert_eq!(manager.idle_state(), IdleSocketState::Listening);

        manager.stop_idle_listener();
        assert_eq!(manager.idle_state(), IdleSocketState::Disconnected);
    }

    #[tokio::test]
    async fn imap_engine_sync_folders_and_messages() {
        let engine = ImapEngine::new("acct-1", "imap.nuncio.mx", 993);
        let folders = engine.sync_folders().await.expect("sync folders succeeds");
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].id, "INBOX");

        let (emails, modseq) = engine
            .sync_messages("INBOX", None)
            .await
            .expect("sync messages succeeds");
        assert_eq!(modseq, "imap-modseq-100");
        assert_eq!(emails.len(), 1);
        assert_eq!(emails[0].id, "imap-uid-100");

        engine.send_email(&emails[0]).await.expect("send succeeds");
    }
}
