//! Async Tokio SMTP transport engine wrapping the `lettre` library.

use async_trait::async_trait;
use lettre::message::{
    header::ContentType, Mailbox, Message, MessageBuilder, MultiPart, SinglePart,
};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::Error as LettreSmtpError;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use nuncio_core::model::{Attachment, Email};
use nuncio_core::TlsMode;

use crate::backend::{MessageSender, OutboundMessage};
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
    /// Create a new [`SmtpTransportEngine`] with initialized transport client
    /// for a specific transport security [`TlsMode`].
    pub fn new(
        host: &str,
        port: u16,
        tls_mode: TlsMode,
        username: &str,
        password: &str,
    ) -> Result<Self, MailError> {
        let transport = Self::build_transport(host, port, tls_mode, username, password)?;
        Ok(Self { transport })
    }

    /// Probe the SMTP endpoint: open a real connection to the configured
    /// relay (performing the implicit-TLS wrap or STARTTLS upgrade the
    /// configured [`TlsMode`] selects) and confirm the server responds. Backs
    /// the daemon's `TestAccountConnection` SMTP leg. Returns `Ok(())` only
    /// when a connection genuinely succeeded -- a dial/handshake failure
    /// surfaces as the real [`MailError`], never a fabricated success.
    ///
    /// This validates transport reachability and TLS negotiation; SMTP has no
    /// standalone authentication probe (credentials are exercised at message
    /// submission time), so this deliberately does not claim to verify auth.
    pub async fn probe_connection(&self) -> Result<(), MailError> {
        match self.transport.test_connection().await {
            Ok(true) => Ok(()),
            Ok(false) => Err(MailError::TransportFailed(
                "SMTP server did not accept the connection probe".to_string(),
            )),
            Err(e) => Err(MailError::from(e)),
        }
    }

    /// Create a new [`SmtpTransportEngine`] from an existing [`AsyncSmtpTransport`] client.
    pub fn with_transport(transport: AsyncSmtpTransport<Tokio1Executor>) -> Self {
        Self { transport }
    }

    /// Send an email message using the inner transport client.
    pub async fn send_email(&self, email: &Email) -> Result<(), MailError> {
        Self::send_email_with_transport(&self.transport, email).await
    }

    /// Send a composed [`OutboundMessage`] using the inner transport client.
    /// Returns `Ok(())` only when the transport genuinely accepted the
    /// message. Logs the send attempt and outcome with the recipient COUNT
    /// only (`to` plus `cc` when present) -- never the addresses themselves.
    pub async fn send_message(&self, message: &OutboundMessage) -> Result<(), MailError> {
        let recipient_count =
            1 + usize::from(message.cc.as_ref().is_some_and(|cc| !cc.trim().is_empty()));
        tracing::info!(recipient_count, "smtp send attempt");

        let msg = Self::build_outbound_mime_message(message)?;
        match self.transport.send(msg).await {
            Ok(_) => {
                tracing::info!(recipient_count, "smtp send succeeded");
                Ok(())
            }
            Err(e) => {
                let err = MailError::from(e);
                tracing::warn!(recipient_count, error = %err, "smtp send failed");
                Err(err)
            }
        }
    }

    /// Send an email message using a provided [`AsyncSmtpTransport`] client
    /// instance. Logs the send attempt and outcome with the recipient COUNT
    /// only -- never the address itself.
    pub async fn send_email_with_transport(
        transport: &AsyncSmtpTransport<Tokio1Executor>,
        email: &Email,
    ) -> Result<(), MailError> {
        let recipient_count = 1;
        tracing::info!(recipient_count, "smtp send attempt");

        let msg = Self::build_mime_message(email)?;
        match transport.send(msg).await {
            Ok(_) => {
                tracing::info!(recipient_count, "smtp send succeeded");
                Ok(())
            }
            Err(e) => {
                let err = MailError::from(e);
                tracing::warn!(recipient_count, error = %err, "smtp send failed");
                Err(err)
            }
        }
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

        Self::finish_message(
            builder,
            email.body_plain.as_deref(),
            email.body_html.as_deref(),
            &email.attachments,
        )
    }

    /// Build an RFC 5322 [`lettre::Message`] from a composed [`OutboundMessage`],
    /// supporting an optional `Cc:` recipient in addition to everything
    /// [`Self::build_mime_message`] supports.
    pub fn build_outbound_mime_message(message: &OutboundMessage) -> Result<Message, MailError> {
        let from_mailbox = message
            .from
            .parse::<Mailbox>()
            .map_err(|e| MailError::ParseFailed(format!("invalid sender address: {}", e)))?;

        let to_mailbox = message
            .to
            .parse::<Mailbox>()
            .map_err(|e| MailError::ParseFailed(format!("invalid recipient address: {}", e)))?;

        let mut builder = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(&message.subject);

        if let Some(cc) = message.cc.as_ref().filter(|cc| !cc.trim().is_empty()) {
            let cc_mailbox = cc
                .parse::<Mailbox>()
                .map_err(|e| MailError::ParseFailed(format!("invalid cc address: {}", e)))?;
            builder = builder.cc(cc_mailbox);
        }

        Self::finish_message(
            builder,
            message.body_plain.as_deref(),
            message.body_html.as_deref(),
            &message.attachments,
        )
    }

    /// Shared multipart/attachment assembly for [`Self::build_mime_message`]
    /// and [`Self::build_outbound_mime_message`]: builds `multipart/mixed`
    /// when attachments are present, `multipart/alternative` for
    /// plain+HTML bodies, or a single part otherwise.
    fn finish_message(
        builder: MessageBuilder,
        body_plain: Option<&str>,
        body_html: Option<&str>,
        attachments: &[Attachment],
    ) -> Result<Message, MailError> {
        let has_attachments = !attachments.is_empty();
        let has_plain = body_plain.is_some_and(|b| !b.is_empty());
        let has_html = body_html.is_some_and(|b| !b.is_empty());

        if has_attachments {
            let initial_mixed = if has_plain && has_html {
                let plain_part = SinglePart::plain(body_plain.unwrap_or_default().to_string());
                let html_part = SinglePart::html(body_html.unwrap_or_default().to_string());
                let alt = MultiPart::alternative()
                    .singlepart(plain_part)
                    .singlepart(html_part);
                MultiPart::mixed().multipart(alt)
            } else if has_html {
                let html_part = SinglePart::html(body_html.unwrap_or_default().to_string());
                MultiPart::mixed().singlepart(html_part)
            } else {
                let plain_part = SinglePart::plain(body_plain.unwrap_or_default().to_string());
                MultiPart::mixed().singlepart(plain_part)
            };

            let mut mixed = initial_mixed;
            for att in attachments {
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
            let plain_part = SinglePart::plain(body_plain.unwrap_or_default().to_string());
            let html_part = SinglePart::html(body_html.unwrap_or_default().to_string());
            let alt = MultiPart::alternative()
                .singlepart(plain_part)
                .singlepart(html_part);
            builder
                .multipart(alt)
                .map_err(|e| MailError::ParseFailed(e.to_string()))
        } else if has_html {
            let html_part = SinglePart::html(body_html.unwrap_or_default().to_string());
            builder
                .singlepart(html_part)
                .map_err(|e| MailError::ParseFailed(e.to_string()))
        } else {
            let plain_part = SinglePart::plain(body_plain.unwrap_or_default().to_string());
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

    /// Build an [`AsyncSmtpTransport`] client instance whose transport
    /// security genuinely follows the configured [`TlsMode`]:
    ///
    /// - [`TlsMode::ImplicitTls`] wraps the socket in TLS immediately
    ///   (`Tls::Wrapper`), as on the SMTPS port (465).
    /// - [`TlsMode::StartTls`] connects in the clear and REQUIRES a STARTTLS
    ///   upgrade before proceeding (`Tls::Required`), as on the submission
    ///   port (587); a server without STARTTLS is rejected rather than
    ///   silently falling back to plaintext.
    /// - [`TlsMode::Plain`] performs no TLS at all (`Tls::None`), for a local
    ///   test/dev relay -- the caller opted into cleartext explicitly.
    ///
    /// Implicit-TLS and STARTTLS relays are built via `relay(host)` (which
    /// defaults to TLS wrapping) / `starttls_relay(host)` respectively;
    /// plaintext is built via the non-encrypting `builder_dangerous(host)`.
    pub fn build_transport(
        host: &str,
        port: u16,
        tls_mode: TlsMode,
        username: &str,
        password: &str,
    ) -> Result<AsyncSmtpTransport<Tokio1Executor>, MailError> {
        Self::validate_smtp_config(host, port, username)?;

        let creds = Credentials::new(username.to_string(), password.to_string());

        let builder = match tls_mode {
            TlsMode::ImplicitTls | TlsMode::StartTls => {
                let tls_params = TlsParameters::builder(host.to_string())
                    .build()
                    .map_err(|e| {
                        MailError::TransportFailed(format!("TLS parameters error: {}", e))
                    })?;
                let tls = match tls_mode {
                    TlsMode::StartTls => Tls::Required(tls_params),
                    _ => Tls::Wrapper(tls_params),
                };
                AsyncSmtpTransport::<Tokio1Executor>::relay(host)
                    .map_err(|e| {
                        MailError::TransportFailed(format!("relay configuration error: {}", e))
                    })?
                    .tls(tls)
            }
            TlsMode::Plain => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
        };

        let transport = builder.port(port).credentials(creds).build();

        Ok(transport)
    }
}

/// Production implementation of [`MessageSender`] for the outbound send RPC:
/// delegates to [`SmtpTransportEngine::send_message`], so a real send
/// genuinely reaches the configured SMTP server -- never fabricated.
#[async_trait]
impl MessageSender for SmtpTransportEngine {
    async fn send(&self, message: &OutboundMessage) -> Result<(), MailError> {
        self.send_message(message).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use nuncio_core::model::Attachment;
    use tracing_test::traced_test;

    fn sample_email() -> Email {
        Email {
            id: "msg-1".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "outbox".to_string(),
            remote_id: "1".to_string(),
            uid_validity: "1".to_string(),
            subject: "Status Update".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: true,
            body_plain: Some("Plaintext status update".to_string()),
            body_html: None,
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
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
    fn build_transport_creates_starttls_client() {
        let transport = SmtpTransportEngine::build_transport(
            "smtp.nuncio.mx",
            587,
            TlsMode::StartTls,
            "user",
            "pass",
        );
        assert!(transport.is_ok());
    }

    #[test]
    fn build_transport_creates_implicit_tls_client() {
        let transport = SmtpTransportEngine::build_transport(
            "smtp.nuncio.mx",
            465,
            TlsMode::ImplicitTls,
            "user",
            "pass",
        );
        assert!(transport.is_ok());
    }

    #[test]
    fn build_transport_creates_plaintext_client() {
        let transport = SmtpTransportEngine::build_transport(
            "smtp.nuncio.mx",
            25,
            TlsMode::Plain,
            "user",
            "pass",
        );
        assert!(transport.is_ok());
    }

    #[tokio::test]
    async fn probe_connection_to_unreachable_server_fails() {
        let engine = SmtpTransportEngine::new("127.0.0.1", 1, TlsMode::ImplicitTls, "user", "pass")
            .expect("valid config");
        let err = engine
            .probe_connection()
            .await
            .expect_err("probe against port 1 must fail, never fabricate success");
        assert!(matches!(
            err,
            MailError::TransportFailed(_) | MailError::SmtpFailed(_)
        ));
    }

    #[tokio::test]
    async fn send_email_to_unreachable_server_fails_with_transport_error() {
        let engine = SmtpTransportEngine::new("127.0.0.1", 1, TlsMode::ImplicitTls, "user", "pass")
            .expect("valid config");
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
        let engine =
            SmtpTransportEngine::new("smtp.nuncio.mx", 465, TlsMode::ImplicitTls, "user", "pass");
        assert!(engine.is_ok());
    }

    fn sample_outbound_message() -> OutboundMessage {
        OutboundMessage {
            from: "alice@nuncio.mx".to_string(),
            to: "bob@nuncio.mx".to_string(),
            cc: None,
            subject: "Status Update".to_string(),
            body_plain: Some("Plaintext status update".to_string()),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn build_outbound_mime_message_plain_text_without_cc() {
        let message = sample_outbound_message();
        let msg =
            SmtpTransportEngine::build_outbound_mime_message(&message).expect("build succeeds");
        assert_eq!(msg.headers().get_raw("Subject").unwrap(), "Status Update");
        assert!(msg.headers().get_raw("Cc").is_none());
        let raw_bytes = msg.formatted();
        let formatted = String::from_utf8_lossy(&raw_bytes);
        assert!(formatted.contains("Plaintext status update"));
    }

    #[test]
    fn build_outbound_mime_message_includes_cc_header_when_present() {
        let mut message = sample_outbound_message();
        message.cc = Some("carol@nuncio.mx".to_string());
        let msg =
            SmtpTransportEngine::build_outbound_mime_message(&message).expect("build succeeds");
        let raw_bytes = msg.formatted();
        let formatted = String::from_utf8_lossy(&raw_bytes);
        assert!(formatted.contains("carol@nuncio.mx"));
    }

    #[test]
    fn build_outbound_mime_message_ignores_blank_cc() {
        let mut message = sample_outbound_message();
        message.cc = Some("   ".to_string());
        let msg =
            SmtpTransportEngine::build_outbound_mime_message(&message).expect("build succeeds");
        assert!(msg.headers().get_raw("Cc").is_none());
    }

    #[test]
    fn build_outbound_mime_message_html_and_attachments() {
        let mut message = sample_outbound_message();
        message.body_html = Some("<p>HTML update</p>".to_string());
        message.attachments.push(Attachment {
            filename: "report.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            content: Bytes::from_static(b"%PDF-1.4 fake pdf data"),
        });
        let msg =
            SmtpTransportEngine::build_outbound_mime_message(&message).expect("build succeeds");
        let raw_bytes = msg.formatted();
        let formatted = String::from_utf8_lossy(&raw_bytes);
        assert!(formatted.contains("multipart/mixed"));
        assert!(formatted.contains("report.pdf"));
        assert!(formatted.contains("<p>HTML update</p>"));
    }

    #[test]
    fn build_outbound_mime_message_invalid_sender_fails() {
        let mut message = sample_outbound_message();
        message.from = "invalid_sender".to_string();
        let err =
            SmtpTransportEngine::build_outbound_mime_message(&message).expect_err("should fail");
        assert!(matches!(err, MailError::ParseFailed(_)));
    }

    #[test]
    fn build_outbound_mime_message_invalid_recipient_fails() {
        let mut message = sample_outbound_message();
        message.to = "invalid_recipient".to_string();
        let err =
            SmtpTransportEngine::build_outbound_mime_message(&message).expect_err("should fail");
        assert!(matches!(err, MailError::ParseFailed(_)));
    }

    #[test]
    fn build_outbound_mime_message_invalid_cc_fails() {
        let mut message = sample_outbound_message();
        message.cc = Some("invalid_cc".to_string());
        let err =
            SmtpTransportEngine::build_outbound_mime_message(&message).expect_err("should fail");
        assert!(matches!(err, MailError::ParseFailed(_)));
    }

    #[tokio::test]
    async fn send_message_to_unreachable_server_fails_with_transport_error() {
        let engine = SmtpTransportEngine::new("127.0.0.1", 1, TlsMode::ImplicitTls, "user", "pass")
            .expect("valid config");
        let message = sample_outbound_message();
        let err = engine
            .send_message(&message)
            .await
            .expect_err("delivery to port 1 should fail");
        assert!(matches!(
            err,
            MailError::TransportFailed(_) | MailError::SmtpFailed(_)
        ));
    }

    #[traced_test]
    #[tokio::test]
    async fn send_message_logs_attempt_and_failure_with_recipient_count_never_addresses() {
        let engine = SmtpTransportEngine::new("127.0.0.1", 1, TlsMode::ImplicitTls, "user", "pass")
            .expect("valid config");
        let mut message = sample_outbound_message();
        message.cc = Some("carol@nuncio.mx".to_string());
        let _ = engine
            .send_message(&message)
            .await
            .expect_err("delivery to port 1 should fail");

        assert!(logs_contain("smtp send attempt"));
        assert!(logs_contain("recipient_count=2"));
        assert!(logs_contain("smtp send failed"));

        assert!(!logs_contain("alice@nuncio.mx"));
        assert!(!logs_contain("bob@nuncio.mx"));
        assert!(!logs_contain("carol@nuncio.mx"));
        assert!(!logs_contain("Status Update"));
    }

    #[tokio::test]
    async fn message_sender_trait_impl_delegates_to_send_message() {
        // Proves `SmtpTransportEngine`'s `MessageSender` trait impl is wired
        // through to the real `send_message` path, not a separate/fabricated
        // stub -- exercised via the trait object
        // exactly like production code (`nunciod::send`) uses it.
        let engine = SmtpTransportEngine::new("127.0.0.1", 1, TlsMode::ImplicitTls, "user", "pass")
            .expect("valid config");
        let sender: &dyn MessageSender = &engine;
        let message = sample_outbound_message();
        let err = sender
            .send(&message)
            .await
            .expect_err("delivery to port 1 should fail");
        assert!(matches!(
            err,
            MailError::TransportFailed(_) | MailError::SmtpFailed(_)
        ));
    }
}
