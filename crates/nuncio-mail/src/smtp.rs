//! Async Tokio SMTP transport engine wrapping the `lettre` library.

use lettre::message::{header::ContentType, Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use nuncio_core::model::Email;
use std::str::FromStr;

use crate::parser::MailError;

/// Async Tokio SMTP client transport engine.
pub struct SmtpTransportEngine;

impl SmtpTransportEngine {
    /// Build an RFC 5322 [`lettre::Message`] from a Nuncio [`Email`] entity.
    pub fn build_mime_message(email: &Email) -> Result<Message, MailError> {
        let from_mailbox = email
            .sender
            .parse::<Mailbox>()
            .map_err(|e| MailError::ParseFailed(format!("invalid sender address: {}", e)))?;

        let to_mailbox = email
            .recipient
            .parse::<Mailbox>()
            .map_err(|e| MailError::ParseFailed(format!("invalid recipient address: {}", e)))?;

        let builder = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(&email.subject);

        let message = if let Some(html) = &email.body_html {
            builder
                .header(ContentType::TEXT_HTML)
                .body(html.clone())
                .map_err(|e| MailError::ParseFailed(e.to_string()))?
        } else {
            let plain = email.body_plain.as_deref().unwrap_or("");
            builder
                .header(ContentType::TEXT_PLAIN)
                .body(plain.to_string())
                .map_err(|e| MailError::ParseFailed(e.to_string()))?
        };

        Ok(message)
    }

    /// Validate SMTP server host, port, and credentials.
    pub fn validate_smtp_config(host: &str, port: u16, username: &str) -> Result<(), MailError> {
        if host.trim().is_empty() {
            return Err(MailError::ParseFailed("SMTP host cannot be empty".to_string()));
        }
        if port == 0 {
            return Err(MailError::ParseFailed("invalid SMTP port number".to_string()));
        }
        if username.trim().is_empty() {
            return Err(MailError::ParseFailed("SMTP username cannot be empty".to_string()));
        }
        Ok(())
    }

    /// Build an [`AsyncSmtpTransport`] client instance.
    pub fn build_transport(
        host: &str,
        port: u16,
        username: &str,
        password: &str,
    ) -> Result<AsyncSmtpTransport<Tokio1Executor>, MailError> {
        Self::validate_smtp_config(host, port, username)?;

        let creds = Credentials::new(username.to_string(), password.to_string());
        let tls_params = TlsParameters::builder(host.to_string())
            .build()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;

        let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(host)
            .map_err(|e| MailError::ParseFailed(e.to_string()))?
            .port(port)
            .credentials(creds)
            .tls(Tls::Required(tls_params))
            .build();

        Ok(transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn sample_email() -> Email {
        Email {
            id: "msg-1".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "outbox".to_string(),
            subject: "Status Update".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: true,
            body_plain: Some("Plaintext status update".to_string()),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn build_mime_message_plain_text() {
        let email = sample_email();
        let msg = SmtpTransportEngine::build_mime_message(&email).expect("build succeeds");
        assert_eq!(msg.headers().get_raw("Subject").unwrap(), "Status Update");
    }

    #[test]
    fn build_mime_message_html() {
        let mut email = sample_email();
        email.body_html = Some("<p>HTML status update</p>".to_string());
        let msg = SmtpTransportEngine::build_mime_message(&email).expect("build succeeds");
        assert_eq!(msg.headers().get_raw("Subject").unwrap(), "Status Update");
    }

    #[test]
    fn build_mime_message_invalid_sender_fails() {
        let mut email = sample_email();
        email.sender = "invalid_sender".to_string();
        let err = SmtpTransportEngine::build_mime_message(&email).expect_err("should fail");
        assert!(matches!(err, MailError::ParseFailed(_)));
    }

    #[test]
    fn validate_smtp_config_checks() {
        assert!(SmtpTransportEngine::validate_smtp_config("smtp.nuncio.mx", 587, "user").is_ok());

        assert_eq!(
            SmtpTransportEngine::validate_smtp_config(" ", 587, "user").unwrap_err(),
            MailError::ParseFailed("SMTP host cannot be empty".to_string())
        );

        assert_eq!(
            SmtpTransportEngine::validate_smtp_config("smtp.nuncio.mx", 0, "user").unwrap_err(),
            MailError::ParseFailed("invalid SMTP port number".to_string())
        );

        assert_eq!(
            SmtpTransportEngine::validate_smtp_config("smtp.nuncio.mx", 587, "").unwrap_err(),
            MailError::ParseFailed("SMTP username cannot be empty".to_string())
        );
    }

    #[test]
    fn build_transport_creates_client() {
        let transport = SmtpTransportEngine::build_transport("smtp.nuncio.mx", 587, "user", "pass");
        assert!(transport.is_ok());
    }
}
