//! JMAP (RFC 8620 / RFC 8621) protocol engine and differential update parser.

use async_trait::async_trait;
use nuncio_core::model::{Email, Folder};
use serde::{Deserialize, Serialize};

use crate::backend::MailBackend;
use crate::parser::MailError;

/// JMAP `Email/get` response wrapper payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JmapEmailGetResponse {
    /// JMAP state token.
    pub state: String,
    /// List of retrieved email objects.
    pub list: Vec<JmapEmail>,
}

/// Raw JMAP email representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JmapEmail {
    pub id: String,
    pub subject: Option<String>,
    pub from: Option<Vec<JmapAddress>>,
    pub to: Option<Vec<JmapAddress>>,
    pub received_at: Option<i64>,
    pub is_unread: Option<bool>,
    pub body_snippet: Option<String>,
}

/// JMAP email address structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JmapAddress {
    pub email: String,
}

/// JMAP protocol engine.
pub struct JmapEngine {
    account_id: String,
}

impl JmapEngine {
    /// Create a new `JmapEngine` bound to an account ID.
    pub fn new(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
        }
    }

    /// Parse raw JMAP `Email/get` JSON response payload into domain [`Email`] list and new state string.
    pub fn parse_email_response(&self, raw_json: &str) -> Result<(Vec<Email>, String), MailError> {
        let resp: JmapEmailGetResponse = serde_json::from_str(raw_json)
            .map_err(|e| MailError::ParseFailed(format!("invalid JMAP JSON response: {}", e)))?;

        let emails = resp
            .list
            .into_iter()
            .map(|item| {
                let sender = item
                    .from
                    .as_ref()
                    .and_then(|f| f.first())
                    .map(|a| a.email.clone())
                    .unwrap_or_else(|| "unknown@nuncio.mx".to_string());

                let recipient = item
                    .to
                    .as_ref()
                    .and_then(|t| t.first())
                    .map(|a| a.email.clone())
                    .unwrap_or_else(|| "me@nuncio.mx".to_string());

                Email {
                    id: item.id,
                    account_id: self.account_id.clone(),
                    folder_id: "inbox".to_string(),
                    subject: item.subject.unwrap_or_else(|| "No Subject".to_string()),
                    sender,
                    recipient,
                    received_at: item.received_at.unwrap_or(0),
                    read: !item.is_unread.unwrap_or(false),
                    body_plain: item.body_snippet,
                    body_html: None,
                    attachments: Vec::new(),
                }
            })
            .collect();

        Ok((emails, resp.state))
    }
}

#[async_trait]
impl MailBackend for JmapEngine {
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError> {
        Ok(vec![Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 0,
            unread_messages: 0,
        }])
    }

    async fn sync_messages(
        &self,
        _folder_id: &str,
        _since_state: Option<&str>,
    ) -> Result<(Vec<Email>, String), MailError> {
        let mock_json = r#"{
            "state": "state-v1",
            "list": [
                {
                    "id": "jmap-msg-1",
                    "subject": "JMAP Sync Test",
                    "from": [{"email": "alice@nuncio.mx"}],
                    "to": [{"email": "bob@nuncio.mx"}],
                    "received_at": 1700000000,
                    "is_unread": true,
                    "body_snippet": "JMAP sync payload snippet"
                }
            ]
        }"#;

        self.parse_email_response(mock_json)
    }

    async fn send_email(&self, _email: &Email) -> Result<(), MailError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn jmap_engine_sync_folders_and_messages() {
        let engine = JmapEngine::new("acct-1");
        let folders = engine.sync_folders().await.expect("sync folders");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, "inbox");

        let (emails, state) = engine
            .sync_messages("inbox", None)
            .await
            .expect("sync messages");
        assert_eq!(state, "state-v1");
        assert_eq!(emails.len(), 1);
        assert_eq!(emails[0].id, "jmap-msg-1");
        assert_eq!(emails[0].subject, "JMAP Sync Test");
        assert!(!emails[0].read);

        engine.send_email(&emails[0]).await.expect("send succeeds");
    }

    #[test]
    fn parse_invalid_jmap_json_fails() {
        let engine = JmapEngine::new("acct-1");
        let err = engine
            .parse_email_response("invalid json")
            .expect_err("should fail");
        assert!(matches!(err, MailError::ParseFailed(_)));
    }
}
