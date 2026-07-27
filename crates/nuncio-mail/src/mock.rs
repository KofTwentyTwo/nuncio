//! Deterministic mock mail backend for offline testing and integration verification.

use async_trait::async_trait;
use nuncio_core::model::{Email, Folder};
use std::sync::{Arc, Mutex};

use crate::backend::{MailBackend, MessageSender, OutboundMessage};
use crate::parser::MailError;

/// Thread-safe mock mail backend for offline testing.
#[derive(Debug, Clone, Default)]
pub struct MockMailBackend {
    folders: Arc<Mutex<Vec<Folder>>>,
    messages: Arc<Mutex<Vec<Email>>>,
    sent_messages: Arc<Mutex<Vec<Email>>>,
    should_fail: Arc<Mutex<bool>>,
}

impl MockMailBackend {
    /// Create a new `MockMailBackend` with empty storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the mock to simulate network failure errors.
    pub fn set_should_fail(&self, fail: bool) {
        if let Ok(mut flag) = self.should_fail.lock() {
            *flag = fail;
        }
    }

    /// Add a mock folder to the storage.
    pub fn add_folder(&self, folder: Folder) {
        if let Ok(mut guard) = self.folders.lock() {
            guard.push(folder);
        }
    }

    /// Add a mock email message to the storage.
    pub fn add_message(&self, email: Email) {
        if let Ok(mut guard) = self.messages.lock() {
            guard.push(email);
        }
    }

    /// Retrieve sent messages recorded by the mock.
    pub fn sent_messages(&self) -> Vec<Email> {
        self.sent_messages
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl MailBackend for MockMailBackend {
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(MailError::ParseFailed(
                "simulated network failure".to_string(),
            ));
        }
        let folders = self
            .folders
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        Ok(folders.clone())
    }

    async fn sync_messages(
        &self,
        folder_id: &str,
        _since_state: Option<&str>,
    ) -> Result<(Vec<Email>, String), MailError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(MailError::ParseFailed(
                "simulated network failure".to_string(),
            ));
        }
        let messages = self
            .messages
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        let matches = messages
            .iter()
            .filter(|m| m.folder_id == folder_id)
            .cloned()
            .collect();
        Ok((matches, "mock-state-token-100".to_string()))
    }

    async fn send_email(&self, email: &Email) -> Result<(), MailError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(MailError::ParseFailed(
                "simulated network failure".to_string(),
            ));
        }
        let mut sent = self
            .sent_messages
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        sent.push(email.clone());
        Ok(())
    }
}

/// Deterministic mock [`MessageSender`] for offline testing of the outbound
/// send RPC (backlog story 1.C.5, GH #160): records every [`OutboundMessage`]
/// it is given, rather than touching a real SMTP transport, and can be
/// configured to simulate a transport failure so callers can prove a failed
/// send surfaces as a genuine error rather than a fabricated success.
#[derive(Debug, Clone, Default)]
pub struct MockMessageSender {
    sent: Arc<Mutex<Vec<OutboundMessage>>>,
    should_fail: Arc<Mutex<bool>>,
}

impl MockMessageSender {
    /// Create a new `MockMessageSender` with empty storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the mock to simulate a transport-level send failure.
    pub fn set_should_fail(&self, fail: bool) {
        if let Ok(mut flag) = self.should_fail.lock() {
            *flag = fail;
        }
    }

    /// Retrieve every message recorded by the mock, in send order.
    pub fn sent_messages(&self) -> Vec<OutboundMessage> {
        self.sent
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl MessageSender for MockMessageSender {
    async fn send(&self, message: &OutboundMessage) -> Result<(), MailError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(MailError::TransportFailed(
                "simulated SMTP transport failure".to_string(),
            ));
        }
        let mut sent = self
            .sent
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        sent.push(message.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_backend_folder_and_message_operations() {
        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 1,
            unread_messages: 1,
        });

        let email = Email {
            id: "msg-mock-1".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "inbox".to_string(),
            subject: "Mock Test".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: false,
            body_plain: Some("Mock body".to_string()),
            body_html: None,
            attachments: Vec::new(),
        };
        mock.add_message(email.clone());

        let folders = mock.sync_folders().await.expect("sync folders succeeds");
        assert_eq!(folders.len(), 1);

        let (msgs, state) = mock
            .sync_messages("inbox", None)
            .await
            .expect("sync messages succeeds");
        assert_eq!(msgs.len(), 1);
        assert_eq!(state, "mock-state-token-100");

        mock.send_email(&email).await.expect("send succeeds");
        assert_eq!(mock.sent_messages().len(), 1);

        // Test error simulation
        mock.set_should_fail(true);
        assert!(mock.sync_folders().await.is_err());
        assert!(mock.sync_messages("inbox", None).await.is_err());
        assert!(mock.send_email(&email).await.is_err());
    }

    fn sample_outbound_message() -> OutboundMessage {
        OutboundMessage {
            from: "alice@nuncio.mx".to_string(),
            to: "bob@nuncio.mx".to_string(),
            cc: None,
            subject: "Mock Send".to_string(),
            body_plain: Some("Mock body".to_string()),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn mock_message_sender_records_exact_message_and_supports_failure_simulation() {
        let mock = MockMessageSender::new();
        let message = sample_outbound_message();

        mock.send(&message).await.expect("send succeeds");
        let sent = mock.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], message);

        mock.set_should_fail(true);
        let err = mock
            .send(&message)
            .await
            .expect_err("simulated failure must surface as an error");
        assert!(matches!(err, MailError::TransportFailed(_)));
        // The failed attempt must never be recorded as "sent".
        assert_eq!(mock.sent_messages().len(), 1);
    }
}
