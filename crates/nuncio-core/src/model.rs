//! Pure domain entities owned by Nuncio (Email, Attachment, Folder).

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Email attachment metadata and payload buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Attachment filename.
    pub filename: String,
    /// MIME content type (e.g. "application/pdf").
    pub mime_type: String,
    /// Raw attachment content buffer.
    #[serde(skip)]
    pub content: Bytes,
}

/// Email message domain entity owned by Nuncio core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Email {
    /// Unique message identifier.
    pub id: String,
    /// Account identifier owning the message.
    pub account_id: String,
    /// Mailbox folder identifier (e.g. "inbox").
    pub folder_id: String,
    /// Subject line.
    pub subject: String,
    /// Sender address (e.g. "alice@nuncio.mx").
    pub sender: String,
    /// Recipient address (e.g. "bob@nuncio.mx").
    pub recipient: String,
    /// Unix timestamp of message arrival.
    pub received_at: i64,
    /// Read/unread flag status.
    pub read: bool,
    /// Plaintext message body.
    pub body_plain: Option<String>,
    /// HTML message body.
    pub body_html: Option<String>,
    /// List of attached files.
    pub attachments: Vec<Attachment>,
}

/// Mailbox folder entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// Folder identifier.
    pub id: String,
    /// Folder display name.
    pub name: String,
    /// Total message count.
    pub total_messages: usize,
    /// Unread message count.
    pub unread_messages: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_domain_entity_creation_and_serde() {
        let email = Email {
            id: "msg-100".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "inbox".to_string(),
            subject: "Architecture Review".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: false,
            body_plain: Some("Hello world".to_string()),
            body_html: Some("<p>Hello world</p>".to_string()),
            attachments: vec![Attachment {
                filename: "spec.pdf".to_string(),
                mime_type: "application/pdf".to_string(),
                content: Bytes::from_static(b"PDF DATA"),
            }],
        };

        let json = serde_json::to_string(&email).unwrap();
        let parsed: Email = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, email.id);
        assert_eq!(parsed.subject, email.subject);
    }
}
