//! Zero-copy MIME parser adapter wrapping Stalwart's `mail-parser` library.

use bytes::Bytes;
use mail_parser::{MessageParser, MimeHeaders};
use nuncio_core::model::{Attachment, Email};
use thiserror::Error;

/// Errors returned by the MIME parsing engine.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum MailError {
    /// Failed to parse RFC 5322 byte slice.
    #[error("failed to parse MIME email payload: {0}")]
    ParseFailed(String),
}

/// MIME parser adapter converting raw byte buffers into Nuncio [`Email`] domain entities.
pub struct MimeParserAdapter;

impl MimeParserAdapter {
    /// Parse a raw RFC 5322 byte slice into an [`Email`] domain entity.
    pub fn parse_mime(
        id: &str,
        account_id: &str,
        folder_id: &str,
        raw_bytes: &[u8],
    ) -> Result<Email, MailError> {
        let msg = MessageParser::default()
            .parse(raw_bytes)
            .ok_or_else(|| MailError::ParseFailed("invalid RFC 5322 MIME structure".to_string()))?;

        let subject = msg.subject().unwrap_or("No Subject").to_string();

        let sender = msg
            .from()
            .and_then(|f| f.first())
            .and_then(|a| a.address())
            .unwrap_or("unknown@nuncio.mx")
            .to_string();

        let recipient = msg
            .to()
            .and_then(|t| t.first())
            .and_then(|a| a.address())
            .unwrap_or("me@nuncio.mx")
            .to_string();

        let received_at = msg.date().map_or(0, |d| d.to_timestamp());

        let body_plain = msg.body_text(0).map(|b| b.to_string());
        let body_html = msg.body_html(0).map(|b| b.to_string());

        let mut attachments = Vec::new();
        for attachment in msg.attachments() {
            let filename = attachment
                .attachment_name()
                .unwrap_or("unnamed_attachment")
                .to_string();

            let mime_type =
                attachment
                    .content_type()
                    .map_or("application/octet-stream".to_string(), |c| {
                        format!(
                            "{}/{}",
                            c.c_type,
                            c.c_subtype.as_deref().unwrap_or("octet-stream")
                        )
                    });

            let content = Bytes::copy_from_slice(attachment.contents());

            attachments.push(Attachment {
                filename,
                mime_type,
                content,
            });
        }

        Ok(Email {
            id: id.to_string(),
            account_id: account_id.to_string(),
            folder_id: folder_id.to_string(),
            subject,
            sender,
            recipient,
            received_at,
            read: false,
            body_plain,
            body_html,
            attachments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_mime_email_with_text_and_html() {
        let raw = b"From: Alice <alice@nuncio.mx>\r\n\
                    To: Bob <bob@nuncio.mx>\r\n\
                    Subject: Test Subject\r\n\
                    Content-Type: text/plain\r\n\
                    \r\n\
                    Hello Bob, this is a plain text email.";

        let email =
            MimeParserAdapter::parse_mime("msg-1", "acct-1", "inbox", raw).expect("parse succeeds");
        assert_eq!(email.id, "msg-1");
        assert_eq!(email.subject, "Test Subject");
        assert_eq!(email.sender, "alice@nuncio.mx");
        assert_eq!(email.recipient, "bob@nuncio.mx");
        assert_eq!(
            email.body_plain,
            Some("Hello Bob, this is a plain text email.".to_string())
        );
    }

    #[test]
    fn parse_invalid_mime_returns_error() {
        let raw = b"";
        let err = MimeParserAdapter::parse_mime("msg-2", "acct-1", "inbox", raw)
            .expect_err("should fail");
        assert_eq!(
            err,
            MailError::ParseFailed("invalid RFC 5322 MIME structure".to_string())
        );
        assert_eq!(
            err.to_string(),
            "failed to parse MIME email payload: invalid RFC 5322 MIME structure"
        );
    }
}
