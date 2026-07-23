//! Async Tokio SMTP transport engine wrapping the `lettre` library.

use lettre::message::{header::ContentType, Mailbox, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::Error as LettreSmtpError;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use nuncio_core::model::Email;

use crate::parser::MailError;

impl From<LettreSmtpError> for MailError {
    fn from(err: LettreSmtpError) -> Self {
        let err_str = err.to_string();
        if err_str.contains("535")
            || err_str.to_lowercase().contains("authentication")
            || err_str.to_lowercase().contains("auth")
            || err_str.to_lowercase().contains("credentials")
        {
            MailError::AuthenticationFailed(err_str)
        } else if err_str.to_lowercase().contains("connect")
            || err_str.to_lowercase().contains("tls")
            || err_str.to_lowercase().contains("timed out")
            || err_str.to_lowercase().contains("handshake")
            || err_str.to_lowercase().contains("invalid")
            || err_str.to_lowercase().contains("dns")
        {
            MailError::TransportFailed(err_str)
        } else {
            MailError::SmtpFailed(err_str)
        }
    }
}

/// Async Tokio SMTP client transport engine.
pub struct SmtpTransportEngine {
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl SmtpTransportEngine {
    /// Create a new [`SmtpTransportEngine`] with initialized transport client.
    pub fn new(host: &str, port: u16, username: &str, password: &str) -> Result<Self, MailError> {
        let transport = Self::build_transport(host, port, username, password)?;
        Ok(Self { transport })
    }

    /// Create a new [`SmtpTransportEngine`] from an existing [`AsyncSmtpTransport`] client.
    pub fn with_transport(transport: AsyncSmtpTransport<Tokio1Executor>) -> Self {
        Self { transport }
    }

    /// Send an email message using the inner transport client.
    pub async fn send_email(&self, email: &Email) -> Result<(), MailError> {
        Self::send_email_with_transport(&self.transport, email).await
    }

    /// Send an email message using a provided [`AsyncSmtpTransport`] client instance.
    pub async fn send_email_with_transport(
        transport: &AsyncSmtpTransport<Tokio1Executor>,
        email: &Email,
    ) -> Result<(), MailError> {
        let msg = Self::build_mime_message(email)?;
        transport.send(msg).await.map_err(MailError::from)?;
        Ok(())
    }

    /// Build an RFC 5322 [`lettre::Message`] from a Nuncio [`Email`] entity.
    ///
    /// Supports `multipart/alternative` for text/plain and text/html bodies,
    /// and `multipart/mixed` when attachments are present.
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

        let has_attachments = !email.attachments.is_empty();
        let has_plain = email.body_plain.as_ref().is_some_and(|b| !b.is_empty());
        let has_html = email.body_html.as_ref().is_some_and(|b| !b.is_empty());

        if has_attachments {
            let initial_mixed = if has_plain && has_html {
                let plain_part = SinglePart::plain(email.body_plain.clone().unwrap_or_default());
                let html_part = SinglePart::html(email.body_html.clone().unwrap_or_default());
                let alt = MultiPart::alternative()
                    .singlepart(plain_part)
                    .singlepart(html_part);
                MultiPart::mixed().multipart(alt)
            } else if has_html {
                let html_part = SinglePart::html(email.body_html.clone().unwrap_or_default());
                MultiPart::mixed().singlepart(html_part)
            } else {
                let plain_part = SinglePart::plain(email.body_plain.clone().unwrap_or_default());
                MultiPart::mixed().singlepart(plain_part)
            };

            let mut mixed = initial_mixed;
            for att in &email.attachments {
                let content_type = ContentType::parse(&att.mime_type).map_err(|e| {
                    MailError::ParseFailed(format!(
                        "invalid attachment content-type '{}': {}",
                        att.mime_type, e
                    ))
                })?;
                let att_part = lettre::message::Attachment::new(att.filename.clone())
                    .body(att.content.to_vec(), content_type);
                mixed = mixed.singlepart(att_part);
            }

            builder
                .multipart(mixed)
                .map_err(|e| MailError::ParseFailed(e.to_string()))
        } else if has_plain && has_html {
            let plain_part = SinglePart::plain(email.body_plain.clone().unwrap_or_default());
            let html_part = SinglePart::html(email.body_html.clone().unwrap_or_default());
            let alt = MultiPart::alternative()
                .singlepart(plain_part)
                .singlepart(html_part);
            builder
                .multipart(alt)
                .map_err(|e| MailError::ParseFailed(e.to_string()))
        } else if has_html {
            let html_part = SinglePart::html(email.body_html.clone().unwrap_or_default());
            builder
                .singlepart(html_part)
                .map_err(|e| MailError::ParseFailed(e.to_string()))
        } else {
            let plain_part = SinglePart::plain(email.body_plain.clone().unwrap_or_default());
            builder
                .singlepart(plain_part)
                .map_err(|e| MailError::ParseFailed(e.to_string()))
        }
    }

    /// Validate SMTP server host, port, and credentials.
    pub fn validate_smtp_config(host: &str, port: u16, username: &str) -> Result<(), MailError> {
        if host.trim().is_empty() {
            return Err(MailError::ParseFailed(
                "SMTP host cannot be empty".to_string(),
            ));
        }
        if port == 0 {
            return Err(MailError::ParseFailed(
                "invalid SMTP port number".to_string(),
            ));
        }
        if username.trim().is_empty() {
            return Err(MailError::ParseFailed(
                "SMTP username cannot be empty".to_string(),
            ));
        }
        Ok(())
    }

    /// Build an [`AsyncSmtpTransport`] client instance.
    /// Configures Implicit TLS (`Tls::Wrapper`) for port 465, and STARTTLS (`Tls::Required`) for port 587 or other ports.
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
            .map_err(|e| MailError::TransportFailed(format!("TLS parameters error: {}", e)))?;

        let tls = if port == 465 {
            Tls::Wrapper(tls_params)
        } else {
            Tls::Required(tls_params)
        };

        let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(host)
            .map_err(|e| MailError::TransportFailed(format!("relay configuration error: {}", e)))?
            .port(port)
            .credentials(creds)
            .tls(tls)
            .build();

        Ok(transport)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use nuncio_core::model::Attachment;

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
        let raw_bytes = msg.formatted();
        let formatted = String::from_utf8_lossy(&raw_bytes);
        assert!(formatted.contains("Plaintext status update"));
    }

    #[test]
    fn build_mime_message_html() {
        let mut email = sample_email();
        email.body_plain = None;
        email.body_html = Some("<p>HTML status update</p>".to_string());
        let msg = SmtpTransportEngine::build_mime_message(&email).expect("build succeeds");
        assert_eq!(msg.headers().get_raw("Subject").unwrap(), "Status Update");
        let raw_bytes = msg.formatted();
        let formatted = String::from_utf8_lossy(&raw_bytes);
        assert!(formatted.contains("<p>HTML status update</p>"));
    }

    #[test]
    fn build_mime_message_alternative() {
        let mut email = sample_email();
        email.body_plain = Some("Plain text content".to_string());
        email.body_html = Some("<h1>HTML Content</h1>".to_string());
        let msg = SmtpTransportEngine::build_mime_message(&email).expect("build succeeds");
        let raw_bytes = msg.formatted();
        let formatted = String::from_utf8_lossy(&raw_bytes);
        assert!(formatted.contains("multipart/alternative"));
        assert!(formatted.contains("Plain text content"));
        assert!(formatted.contains("<h1>HTML Content</h1>"));
    }

    #[test]
    fn build_mime_message_with_attachments() {
        let mut email = sample_email();
        email.body_plain = Some("Here is your document".to_string());
        email.body_html = Some("<p>Here is your document</p>".to_string());
        email.attachments.push(Attachment {
            filename: "report.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            content: Bytes::from_static(b"%PDF-1.4 fake pdf data"),
        });

        let msg = SmtpTransportEngine::build_mime_message(&email).expect("build succeeds");
        let raw_bytes = msg.formatted();
        let formatted = String::from_utf8_lossy(&raw_bytes);
        assert!(formatted.contains("multipart/mixed"));
        assert!(formatted.contains("report.pdf"));
        assert!(formatted.contains("application/pdf"));
    }

    #[test]
    fn build_mime_message_invalid_sender_fails() {
        let mut email = sample_email();
        email.sender = "invalid_sender".to_string();
        let err = SmtpTransportEngine::build_mime_message(&email).expect_err("should fail");
        assert!(matches!(err, MailError::ParseFailed(_)));
    }

    #[test]
    fn build_mime_message_invalid_recipient_fails() {
        let mut email = sample_email();
        email.recipient = "invalid_recipient".to_string();
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
    fn build_transport_creates_starttls_client_for_port_587() {
        let transport = SmtpTransportEngine::build_transport("smtp.nuncio.mx", 587, "user", "pass");
        assert!(transport.is_ok());
    }

    #[test]
    fn build_transport_creates_implicit_tls_client_for_port_465() {
        let transport = SmtpTransportEngine::build_transport("smtp.nuncio.mx", 465, "user", "pass");
        assert!(transport.is_ok());
    }

    #[tokio::test]
    async fn send_email_to_unreachable_server_fails_with_transport_error() {
        let engine =
            SmtpTransportEngine::new("127.0.0.1", 1, "user", "pass").expect("valid config");
        let email = sample_email();
        let err = engine
            .send_email(&email)
            .await
            .expect_err("delivery to port 1 should fail");
        assert!(matches!(
            err,
            MailError::TransportFailed(_) | MailError::SmtpFailed(_)
        ));
    }

    #[test]
    fn engine_constructor_creates_instance() {
        let engine = SmtpTransportEngine::new("smtp.nuncio.mx", 465, "user", "pass");
        assert!(engine.is_ok());
    }
}
