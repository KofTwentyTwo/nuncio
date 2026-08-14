//! Zero-copy MIME parser adapter wrapping Stalwart's `mail-parser` library.

use bytes::Bytes;
use mail_parser::{MessageParser, MimeHeaders};
use nuncio_core::model::{Attachment, Email};
use thiserror::Error;

/// Errors returned by the MIME parsing engine and transport operations.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum MailError {
    /// Failed to parse RFC 5322 byte slice.
    #[error("failed to parse MIME email payload: {0}")]
    ParseFailed(String),

    /// SMTP transport delivery failed.
    #[error("SMTP transport delivery failed: {0}")]
    SmtpFailed(String),

    /// SMTP authentication failed.
    #[error("SMTP authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Connection or TLS transport setup failed.
    #[error("SMTP transport connection failed: {0}")]
    TransportFailed(String),

    /// TLS connection or handshake failed.
    #[error("TLS connection failed: {0}")]
    TlsError(String),

    /// IMAP connection or protocol error.
    #[error("IMAP error: {0}")]
    ImapError(String),

    /// A single FETCH response item exceeded its bounded read timeout without
    /// the server responding. Distinct from a generic [`MailError::ImapError`]
    /// so callers can tell an honest mid-stream stall (dead socket, server-side
    /// hang) apart from a protocol/parse failure instead of the sync hanging
    /// forever.
    #[error("IMAP FETCH stalled: {0}")]
    FetchStalled(String),

    /// Network or IO error.
    #[error("network I/O error: {0}")]
    NetworkError(String),

    /// Authentication failure.
    #[error("authentication error: {0}")]
    AuthError(String),

    /// General operational failure.
    #[error("operation failed: {0}")]
    OperationFailed(String),
}

/// MIME parser adapter converting raw byte buffers into Nuncio [`Email`] domain entities.
pub struct MimeParserAdapter;

impl MimeParserAdapter {
    /// Maximum allowed MIME payload size (25MB).
    pub const MAX_PAYLOAD_BYTES: usize = 25 * 1024 * 1024;

    /// Parse a raw RFC 5322 byte slice into an [`Email`] domain entity.
    ///
    /// The returned `Email` carries an **empty** `id`, and it is the caller's
    /// job to fill it from [`Email::derive_message_key`]. The derivation needs
    /// inputs this parser cannot see -- the transport's addressing coordinates
    /// and any server-assigned identity (RFC 8474 `EMAILID`, `X-GM-MSGID`) --
    /// alongside the `Message-ID` and content hash recovered here, so the id
    /// can only be settled once both halves are in hand. Minting a provisional
    /// value here would risk one reaching the store as though it were derived.
    pub fn parse_mime(account_id: &str, raw_bytes: &[u8]) -> Result<Email, MailError> {
        if raw_bytes.len() > Self::MAX_PAYLOAD_BYTES {
            return Err(MailError::ParseFailed(
                "MIME payload exceeds maximum allowed limit of 25MB".to_string(),
            ));
        }

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

        let message_id = msg.message_id().and_then(Email::normalize_message_id);
        let content_hash = Some(Email::content_hash_of(raw_bytes));

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
            // Assigned by the caller from the derived message key; see the
            // method doc for why it cannot be settled here.
            id: String::new(),
            account_id: account_id.to_string(),
            subject,
            sender,
            recipient,
            received_at,
            body_plain,
            body_html,
            attachments,
            message_id,
            content_hash,
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

        let email = MimeParserAdapter::parse_mime("acct-1", raw).expect("parse succeeds");
        assert_eq!(
            email.id, "",
            "the parser must not mint an id; the caller derives it"
        );
        assert_eq!(email.account_id, "acct-1");
        assert_eq!(email.subject, "Test Subject");
        assert_eq!(email.sender, "alice@nuncio.mx");
        assert_eq!(email.recipient, "bob@nuncio.mx");
        assert_eq!(
            email.body_plain,
            Some("Hello Bob, this is a plain text email.".to_string())
        );
    }

    #[test]
    fn parse_captures_normalized_message_id_and_content_hash() {
        let raw = b"From: Alice <alice@nuncio.mx>\r\n\
                    To: Bob <bob@nuncio.mx>\r\n\
                    Message-ID: <Abc.123@mail.nuncio.mx>\r\n\
                    Subject: Test\r\n\
                    \r\n\
                    body";

        let email = MimeParserAdapter::parse_mime("acct-1", raw).expect("parse succeeds");
        assert_eq!(email.message_id.as_deref(), Some("Abc.123@mail.nuncio.mx"));
        assert_eq!(
            email.content_hash,
            Some(nuncio_core::model::Email::content_hash_of(raw)),
            "the hash must cover the octets actually parsed"
        );
    }

    #[test]
    fn parse_reports_no_message_id_when_the_header_is_absent() {
        // RFC 5322 3.6.4 makes Message-ID a SHOULD. An absent header must read
        // as "none present", never as an invented or empty value.
        let raw = b"From: Alice <alice@nuncio.mx>\r\n\
                    Subject: No id\r\n\
                    \r\n\
                    body";
        let email = MimeParserAdapter::parse_mime("acct-1", raw).expect("parse succeeds");
        assert_eq!(email.message_id, None);
        assert!(email.content_hash.is_some());
    }

    #[test]
    fn parse_invalid_mime_returns_error() {
        let raw = b"";
        let err = MimeParserAdapter::parse_mime("acct-1", raw).expect_err("should fail");
        assert_eq!(
            err,
            MailError::ParseFailed("invalid RFC 5322 MIME structure".to_string())
        );
        assert_eq!(
            err.to_string(),
            "failed to parse MIME email payload: invalid RFC 5322 MIME structure"
        );
    }

    #[test]
    fn parse_oversized_mime_returns_error() {
        let oversized = vec![0u8; MimeParserAdapter::MAX_PAYLOAD_BYTES + 1];
        let err = MimeParserAdapter::parse_mime("acct-1", &oversized)
            .expect_err("should fail for oversized payload");
        assert!(err
            .to_string()
            .contains("exceeds maximum allowed limit of 25MB"));
    }
}
