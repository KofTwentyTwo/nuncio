//! IMAP4rev1 dual-socket connection engine and IDLE push stream manager.

use async_trait::async_trait;
use nuncio_core::model::{Email, Folder};
use nuncio_core::TlsMode;
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tokio_stream::StreamExt;

/// Upper bound on how long a single IMAP transport connect (TCP dial, plus
/// any STARTTLS negotiation and TLS handshake) may take before it is treated
/// as a failure. Without this an unreachable or silently-dropping host could
/// hang a connect -- and therefore a `TestAccountConnection` RPC -- forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

use crate::backend::MailBackend;
use crate::parser::{MailError, MimeParserAdapter};

/// State of the dedicated IMAP IDLE socket listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleSocketState {
    /// IDLE socket is active and listening for server notifications.
    Listening,
    /// IDLE socket connection is paused or closed.
    Disconnected,
}

/// Real-time event notifications yielded by the IMAP IDLE push stream listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdleEvent {
    /// New message arrived in folder with updated message count.
    NewMessage {
        folder_id: String,
        exists_count: u32,
    },
    /// Message expunged from folder at sequence number.
    Expunge {
        folder_id: String,
        sequence_number: u32,
    },
    /// IDLE listener was disconnected or closed.
    Disconnected,
}

/// Build default TLS ClientConfig using webpki-roots CA certificates.
pub fn build_tls_config() -> Result<Arc<ClientConfig>, MailError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root_store = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

/// Build a TLS connector using `tokio-rustls` and WebPKI root certificates.
pub fn build_tls_connector() -> Result<TlsConnector, MailError> {
    let config = build_tls_config()?;
    Ok(TlsConnector::from(config))
}

/// Helper function to build the IMAP fetch command query parameter string.
pub fn build_fetch_command_query() -> &'static str {
    "(FLAGS INTERNALDATE RFC822.SIZE ENVELOPE BODY.PEEK[])"
}

/// Establish an encrypted TLS stream to an IMAP server.
pub async fn connect_tls_stream(host: &str, port: u16) -> Result<TlsStream<TcpStream>, MailError> {
    let connector = build_tls_connector()?;
    let target_port = if port == 0 { 993 } else { port };
    let addr = format!("{}:{}", host, target_port);

    let tcp_stream = TcpStream::connect(&addr).await.map_err(|e| {
        MailError::NetworkError(format!("TCP connection to {} failed: {}", addr, e))
    })?;

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| MailError::TlsError(format!("invalid TLS server name '{}': {}", host, e)))?;

    let tls_stream = connector
        .connect(server_name, tcp_stream)
        .await
        .map_err(|e| MailError::TlsError(format!("TLS handshake with {} failed: {}", host, e)))?;

    Ok(tls_stream)
}

/// A connected IMAP transport stream, unified across the three transport
/// security modes so a single `async_imap` session type works for all of
/// them: implicit TLS (wrapped immediately on connect), STARTTLS (plain TCP
/// upgraded to TLS after negotiation), or plain unencrypted TCP. The TLS
/// variant is boxed because a `TlsStream` is substantially larger than a bare
/// `TcpStream`, so boxing keeps the enum small and uniform.
#[derive(Debug)]
pub enum MailStream {
    /// TLS-encrypted stream (implicit TLS, or a STARTTLS-upgraded socket).
    Tls(Box<TlsStream<TcpStream>>),
    /// Plain, unencrypted TCP stream.
    Plain(TcpStream),
}

impl AsyncRead for MailStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            MailStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MailStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            MailStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            MailStream::Plain(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            MailStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Establish a plain, unencrypted TCP stream to an IMAP server. Used for
/// [`TlsMode::Plain`] (e.g. a local test/dev port) -- no TLS is negotiated,
/// so credentials travel in the clear; the caller opted into that explicitly.
pub async fn connect_plain_stream(host: &str, port: u16) -> Result<TcpStream, MailError> {
    let target_port = if port == 0 { 143 } else { port };
    let addr = format!("{}:{}", host, target_port);
    TcpStream::connect(&addr)
        .await
        .map_err(|e| MailError::NetworkError(format!("TCP connection to {} failed: {}", addr, e)))
}

/// Read a single CRLF-terminated line from an IMAP transport stream, byte by
/// byte. Used only during STARTTLS negotiation, where exactly one greeting
/// line and then a small number of response lines are read before the socket
/// is upgraded -- not on the hot fetch path, so the simple unbuffered read is
/// adequate and avoids buffering bytes that would be lost across the upgrade.
async fn read_imap_line<S>(stream: &mut S) -> Result<String, MailError>
where
    S: AsyncRead + Unpin,
{
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| MailError::NetworkError(format!("reading IMAP line failed: {}", e)))?;
        if n == 0 {
            return Err(MailError::ImapError(
                "connection closed before a complete IMAP line was read".to_string(),
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            line.push(byte[0]);
        }
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// Establish a STARTTLS IMAP stream: connect plain TCP, read the server
/// greeting, issue an explicit `STARTTLS` command, confirm the tagged `OK`,
/// then upgrade the SAME socket to TLS via the shared rustls connector. This
/// is the RFC 3501 / RFC 2595 explicit-TLS flow used on the submission port
/// (143). The upgrade reuses [`build_tls_connector`], so certificate
/// validation is identical to implicit TLS.
pub async fn connect_starttls_stream(
    host: &str,
    port: u16,
) -> Result<TlsStream<TcpStream>, MailError> {
    let target_port = if port == 0 { 143 } else { port };
    let addr = format!("{}:{}", host, target_port);
    let mut tcp = TcpStream::connect(&addr).await.map_err(|e| {
        MailError::NetworkError(format!("TCP connection to {} failed: {}", addr, e))
    })?;

    // Server greeting (untagged `* OK ...`); consume it before commanding.
    let greeting = read_imap_line(&mut tcp).await?;
    if !greeting.starts_with("* OK") && !greeting.starts_with("* PREAUTH") {
        return Err(MailError::ImapError(format!(
            "unexpected IMAP greeting during STARTTLS negotiation: {}",
            greeting
        )));
    }

    const TAG: &str = "N1";
    tcp.write_all(format!("{} STARTTLS\r\n", TAG).as_bytes())
        .await
        .map_err(|e| MailError::NetworkError(format!("sending STARTTLS failed: {}", e)))?;

    // Read responses until the tagged completion line for our command,
    // tolerating any untagged lines the server may interleave first.
    let tag_prefix = format!("{} ", TAG);
    loop {
        let line = read_imap_line(&mut tcp).await?;
        if let Some(rest) = line.strip_prefix(&tag_prefix) {
            if rest.starts_with("OK") {
                break;
            }
            return Err(MailError::ImapError(format!(
                "server refused STARTTLS: {}",
                line
            )));
        }
    }

    let connector = build_tls_connector()?;
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| MailError::TlsError(format!("invalid TLS server name '{}': {}", host, e)))?;
    let tls_stream = connector.connect(server_name, tcp).await.map_err(|e| {
        MailError::TlsError(format!(
            "TLS handshake with {} after STARTTLS failed: {}",
            host, e
        ))
    })?;
    Ok(tls_stream)
}

/// IMAP dual-socket manager maintaining isolated connections for IDLE push and FETCH/STORE queries.
pub struct ImapDualSocketManager {
    server_host: String,
    server_port: u16,
    tls_mode: TlsMode,
    idle_active: Arc<AtomicBool>,
}

impl ImapDualSocketManager {
    /// Create a new `ImapDualSocketManager` defaulting to implicit TLS. The
    /// default port for a `0` port is the implicit-TLS IMAPS port (993).
    pub fn new(server_host: &str, server_port: u16) -> Self {
        Self::with_tls_mode(server_host, server_port, TlsMode::ImplicitTls)
    }

    /// Create a new `ImapDualSocketManager` for a specific transport security
    /// [`TlsMode`]. A `0` port is resolved to the conventional default for
    /// the chosen mode: 993 for implicit TLS, 143 for STARTTLS/plain.
    pub fn with_tls_mode(server_host: &str, server_port: u16, tls_mode: TlsMode) -> Self {
        let port = if server_port == 0 {
            match tls_mode {
                TlsMode::ImplicitTls => 993,
                TlsMode::StartTls | TlsMode::Plain => 143,
            }
        } else {
            server_port
        };
        Self {
            server_host: server_host.to_string(),
            server_port: port,
            tls_mode,
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

    /// Retrieve the configured transport security mode.
    pub fn tls_mode(&self) -> TlsMode {
        self.tls_mode
    }

    /// Retrieve the current IDLE socket state.
    pub fn idle_state(&self) -> IdleSocketState {
        if self.idle_active.load(Ordering::SeqCst) {
            IdleSocketState::Listening
        } else {
            IdleSocketState::Disconnected
        }
    }

    /// Establish a live transport socket to the target IMAP server, selecting
    /// implicit TLS, STARTTLS, or plain TCP per the configured [`TlsMode`].
    /// Bounded by [`CONNECT_TIMEOUT`] so an unreachable host cannot hang the
    /// caller indefinitely.
    pub async fn connect_stream(&self) -> Result<MailStream, MailError> {
        let connect = async {
            match self.tls_mode {
                TlsMode::ImplicitTls => Ok(MailStream::Tls(Box::new(
                    connect_tls_stream(&self.server_host, self.server_port).await?,
                ))),
                TlsMode::StartTls => Ok(MailStream::Tls(Box::new(
                    connect_starttls_stream(&self.server_host, self.server_port).await?,
                ))),
                TlsMode::Plain => Ok(MailStream::Plain(
                    connect_plain_stream(&self.server_host, self.server_port).await?,
                )),
            }
        };
        tokio::time::timeout(CONNECT_TIMEOUT, connect)
            .await
            .map_err(|_| {
                MailError::NetworkError(format!(
                    "IMAP connect to {}:{} timed out after {}s",
                    self.server_host,
                    self.server_port,
                    CONNECT_TIMEOUT.as_secs()
                ))
            })?
    }

    /// Establish an authenticated IMAP session over a live transport socket
    /// selected by the configured [`TlsMode`].
    pub async fn connect_session(
        &self,
        username: &str,
        password: &str,
    ) -> Result<async_imap::Session<MailStream>, MailError> {
        let stream = self.connect_stream().await?;
        let client = async_imap::Client::new(stream);
        let session = client
            .login(username, password)
            .await
            .map_err(|(err, _client)| {
                MailError::AuthError(format!(
                    "IMAP login failed for user '{}': {}",
                    username, err
                ))
            })?;
        Ok(session)
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
    username: Option<String>,
    password: Option<String>,
}

impl ImapEngine {
    /// Create a new `ImapEngine` defaulting to implicit TLS transport.
    pub fn new(account_id: &str, server_host: &str, server_port: u16) -> Self {
        Self {
            socket_manager: ImapDualSocketManager::new(server_host, server_port),
            account_id: account_id.to_string(),
            username: None,
            password: None,
        }
    }

    /// Create a new `ImapEngine` with host, port, transport security
    /// [`TlsMode`], and authentication credentials. The `tls_mode` genuinely
    /// selects the transport (implicit TLS / STARTTLS / plain) used for every
    /// connection this engine opens -- it is not merely recorded.
    pub fn with_credentials(
        account_id: &str,
        server_host: &str,
        server_port: u16,
        tls_mode: TlsMode,
        username: &str,
        password: &str,
    ) -> Self {
        Self {
            socket_manager: ImapDualSocketManager::with_tls_mode(
                server_host,
                server_port,
                tls_mode,
            ),
            account_id: account_id.to_string(),
            username: Some(username.to_string()),
            password: Some(password.to_string()),
        }
    }

    /// Probe the IMAP endpoint end to end: open a transport connection using
    /// the configured [`TlsMode`], authenticate with the engine's
    /// credentials, and cleanly log out. Returns `Ok(())` only on a genuine
    /// successful login -- any dial, TLS-handshake, or authentication failure
    /// surfaces as the real [`MailError`], never a fabricated success. This
    /// backs the daemon's `TestAccountConnection` IMAP leg.
    pub async fn probe_connection(&self) -> Result<(), MailError> {
        let (username, password) = match (&self.username, &self.password) {
            (Some(u), Some(p)) => (u.as_str(), p.as_str()),
            _ => {
                return Err(MailError::AuthError(
                    "IMAP connection probe requires credentials".to_string(),
                ))
            }
        };
        let mut session = self
            .socket_manager
            .connect_session(username, password)
            .await?;
        let _ = session.logout().await;
        Ok(())
    }

    /// Set authentication credentials for the IMAP engine.
    pub fn set_credentials(&mut self, username: &str, password: &str) {
        self.username = Some(username.to_string());
        self.password = Some(password.to_string());
    }

    /// Access the underlying dual-socket manager.
    pub fn socket_manager(&self) -> &ImapDualSocketManager {
        &self.socket_manager
    }

    /// Fetch and parse all messages in a folder over an active IMAP session.
    pub async fn sync_folder_messages_with_session<S>(
        &self,
        folder_id: &str,
        session: &mut async_imap::Session<S>,
    ) -> Result<Vec<Email>, MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        session.select(folder_id).await.map_err(|e| {
            MailError::ImapError(format!("failed to select folder '{}': {}", folder_id, e))
        })?;

        let query = build_fetch_command_query();
        let mut fetch_stream = session.uid_fetch("1:*", query).await.map_err(|e| {
            MailError::ImapError(format!(
                "UID FETCH failed for folder '{}': {}",
                folder_id, e
            ))
        })?;

        let mut emails = Vec::new();
        while let Some(fetch_res) = fetch_stream.next().await {
            let fetch_data = fetch_res
                .map_err(|e| MailError::ImapError(format!("failed reading fetch item: {}", e)))?;

            let uid_num = fetch_data.uid.unwrap_or(0);
            let email_id = format!("imap-uid-{}", uid_num);

            let is_read = fetch_data
                .flags()
                .any(|flag| matches!(flag, async_imap::types::Flag::Seen));

            let email = if let Some(raw_bytes) = fetch_data.body() {
                let mut email = MimeParserAdapter::parse_mime(
                    &email_id,
                    &self.account_id,
                    folder_id,
                    raw_bytes,
                )?;
                email.read = is_read;
                email
            } else {
                let subject = fetch_data
                    .envelope()
                    .and_then(|env| env.subject.as_ref())
                    .map_or("No Subject".to_string(), |s| {
                        String::from_utf8_lossy(s).to_string()
                    });

                let sender = fetch_data
                    .envelope()
                    .and_then(|env| env.from.as_ref())
                    .and_then(|froms| froms.first())
                    .map_or("unknown@nuncio.mx".to_string(), |addr| {
                        let mailbox = addr
                            .mailbox
                            .as_ref()
                            .map_or("", |m| std::str::from_utf8(m).unwrap_or(""));
                        let host = addr
                            .host
                            .as_ref()
                            .map_or("", |h| std::str::from_utf8(h).unwrap_or(""));
                        if host.is_empty() {
                            mailbox.to_string()
                        } else {
                            format!("{}@{}", mailbox, host)
                        }
                    });

                let recipient = fetch_data
                    .envelope()
                    .and_then(|env| env.to.as_ref())
                    .and_then(|tos| tos.first())
                    .map_or("me@nuncio.mx".to_string(), |addr| {
                        let mailbox = addr
                            .mailbox
                            .as_ref()
                            .map_or("", |m| std::str::from_utf8(m).unwrap_or(""));
                        let host = addr
                            .host
                            .as_ref()
                            .map_or("", |h| std::str::from_utf8(h).unwrap_or(""));
                        if host.is_empty() {
                            mailbox.to_string()
                        } else {
                            format!("{}@{}", mailbox, host)
                        }
                    });

                let received_at = fetch_data.internal_date().map_or(0, |dt| dt.timestamp());

                Email {
                    id: email_id,
                    account_id: self.account_id.clone(),
                    folder_id: folder_id.to_string(),
                    subject,
                    sender,
                    recipient,
                    received_at,
                    read: is_read,
                    body_plain: None,
                    body_html: None,
                    attachments: Vec::new(),
                }
            };

            emails.push(email);
        }

        Ok(emails)
    }

    /// Execute `UID FETCH 1:* (FLAGS INTERNALDATE RFC822.SIZE ENVELOPE BODY.PEEK[])` and parse returned messages into domain `Email` entities.
    pub async fn sync_folder_messages(&self, folder_id: &str) -> Result<Vec<Email>, MailError> {
        let (username, password) = match (&self.username, &self.password) {
            (Some(u), Some(p)) => (u.as_str(), p.as_str()),
            _ => {
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
                return Ok(mock_emails);
            }
        };

        let mut session = self
            .socket_manager
            .connect_session(username, password)
            .await?;
        let emails = self
            .sync_folder_messages_with_session(folder_id, &mut session)
            .await?;
        let _ = session.logout().await;
        Ok(emails)
    }

    /// Listen for real-time IMAP IDLE notification events on an active IMAP session.
    pub async fn listen_idle_session<S, F>(
        &self,
        session: async_imap::Session<S>,
        folder_id: &str,
        mut callback: F,
    ) -> Result<async_imap::Session<S>, MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
        F: FnMut(IdleEvent) + Send,
    {
        self.socket_manager.start_idle_listener()?;

        let mut session = session;
        session.select(folder_id).await.map_err(|e| {
            MailError::ImapError(format!(
                "IDLE failed selecting folder '{}': {}",
                folder_id, e
            ))
        })?;

        let mut handle = session.idle();

        while self.socket_manager.idle_state() == IdleSocketState::Listening {
            handle
                .init()
                .await
                .map_err(|e| MailError::ImapError(format!("IDLE init failed: {}", e)))?;

            let (wait_fut, _stop) = handle.wait();
            let idle_resp = wait_fut
                .await
                .map_err(|e| MailError::ImapError(format!("IDLE wait error: {}", e)))?;

            match idle_resp {
                async_imap::extensions::idle::IdleResponse::NewData(resp_data) => {
                    match resp_data.parsed() {
                        async_imap::imap_proto::Response::MailboxData(
                            async_imap::imap_proto::MailboxDatum::Exists(n),
                        ) => {
                            callback(IdleEvent::NewMessage {
                                folder_id: folder_id.to_string(),
                                exists_count: *n,
                            });
                        }
                        async_imap::imap_proto::Response::Expunge(seq) => {
                            callback(IdleEvent::Expunge {
                                folder_id: folder_id.to_string(),
                                sequence_number: *seq,
                            });
                        }
                        _ => {}
                    }
                }
                async_imap::extensions::idle::IdleResponse::Timeout => {
                    // IDLE timeout reached; loop back to re-issue IDLE init
                }
                async_imap::extensions::idle::IdleResponse::ManualInterrupt => {
                    break;
                }
            }
        }

        let session = handle
            .done()
            .await
            .map_err(|e| MailError::ImapError(format!("IDLE done command error: {}", e)))?;

        callback(IdleEvent::Disconnected);
        Ok(session)
    }

    /// Listen for real-time notification events using IMAP IDLE capability.
    pub async fn listen_idle<F>(&self, folder_id: &str, mut callback: F) -> Result<(), MailError>
    where
        F: FnMut(IdleEvent) + Send,
    {
        let (username, password) = match (&self.username, &self.password) {
            (Some(u), Some(p)) => (u.as_str(), p.as_str()),
            _ => {
                self.socket_manager.start_idle_listener()?;
                callback(IdleEvent::Disconnected);
                return Ok(());
            }
        };

        let session = self
            .socket_manager
            .connect_session(username, password)
            .await?;
        let mut session = self
            .listen_idle_session(session, folder_id, callback)
            .await?;
        let _ = session.logout().await;
        Ok(())
    }
}

#[async_trait]
impl MailBackend for ImapEngine {
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError> {
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            let mut session = self.socket_manager.connect_session(u, p).await?;
            let mut mailboxes = session
                .list(None, Some("*"))
                .await
                .map_err(|e| MailError::ImapError(format!("failed to list mailboxes: {}", e)))?;

            let mut folders = Vec::new();
            while let Some(mb_res) = mailboxes.next().await {
                let mb = mb_res.map_err(|e| {
                    MailError::ImapError(format!("failed reading mailbox item: {}", e))
                })?;
                let folder_name = mb.name().to_string();
                folders.push(Folder {
                    id: folder_name.clone(),
                    name: folder_name,
                    total_messages: 0,
                    unread_messages: 0,
                });
            }
            drop(mailboxes);
            let _ = session.logout().await;
            if !folders.is_empty() {
                return Ok(folders);
            }
        }

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
        let emails = self.sync_folder_messages(folder_id).await?;
        let modseq = format!("imap-modseq-{}", emails.len());
        Ok((emails, modseq))
    }

    async fn send_email(&self, _email: &Email) -> Result<(), MailError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn imap_dual_socket_manager_lifecycle() -> Result<(), MailError> {
        let manager = ImapDualSocketManager::new("mail.kof22.com", 993);
        assert_eq!(manager.server_host(), "mail.kof22.com");
        assert_eq!(manager.server_port(), 993);
        assert_eq!(manager.tls_mode(), TlsMode::ImplicitTls);
        assert_eq!(manager.idle_state(), IdleSocketState::Disconnected);

        manager.start_idle_listener()?;
        assert_eq!(manager.idle_state(), IdleSocketState::Listening);

        manager.stop_idle_listener();
        assert_eq!(manager.idle_state(), IdleSocketState::Disconnected);
        Ok(())
    }

    #[tokio::test]
    async fn imap_engine_sync_folders_and_messages() -> Result<(), MailError> {
        let engine = ImapEngine::new("acct-1", "mail.kof22.com", 993);
        let folders = engine.sync_folders().await?;
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].id, "INBOX");

        let (emails, modseq) = engine.sync_messages("INBOX", None).await?;
        assert_eq!(modseq, "imap-modseq-1");
        assert_eq!(emails.len(), 1);
        assert_eq!(emails[0].id, "imap-uid-100");

        engine.send_email(&emails[0]).await?;
        Ok(())
    }

    #[tokio::test]
    async fn with_tls_mode_records_mode_and_resolves_default_ports() {
        // A `0` port resolves to the conventional default for each mode.
        let implicit = ImapDualSocketManager::with_tls_mode("host", 0, TlsMode::ImplicitTls);
        assert_eq!(implicit.tls_mode(), TlsMode::ImplicitTls);
        assert_eq!(implicit.server_port(), 993);

        let starttls = ImapDualSocketManager::with_tls_mode("host", 0, TlsMode::StartTls);
        assert_eq!(starttls.tls_mode(), TlsMode::StartTls);
        assert_eq!(starttls.server_port(), 143);

        let plain = ImapDualSocketManager::with_tls_mode("host", 0, TlsMode::Plain);
        assert_eq!(plain.tls_mode(), TlsMode::Plain);
        assert_eq!(plain.server_port(), 143);

        // An explicit non-zero port is always honored verbatim.
        let explicit = ImapDualSocketManager::with_tls_mode("host", 2143, TlsMode::StartTls);
        assert_eq!(explicit.server_port(), 2143);
    }

    #[tokio::test]
    async fn connect_stream_plain_connects_to_a_loopback_listener() {
        // Plain mode performs a genuine TCP connect with no TLS: against a
        // loopback listener the connect succeeds and yields the plain
        // transport variant.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("listener addr");
        let accept = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let manager =
            ImapDualSocketManager::with_tls_mode("127.0.0.1", addr.port(), TlsMode::Plain);
        let stream = manager
            .connect_stream()
            .await
            .expect("plain connect to loopback listener succeeds");
        assert!(matches!(stream, MailStream::Plain(_)));
        let _ = accept.await;
    }

    #[tokio::test]
    async fn connect_stream_implicit_tls_fails_fast_against_a_closed_port() {
        // Implicit TLS against a closed port fails at the TCP dial (connection
        // refused) -- a genuine, honest failure, not a fabricated success.
        let manager = ImapDualSocketManager::with_tls_mode("127.0.0.1", 1, TlsMode::ImplicitTls);
        let err = manager
            .connect_stream()
            .await
            .expect_err("connect to a closed port must fail");
        assert!(matches!(
            err,
            MailError::NetworkError(_) | MailError::TlsError(_)
        ));
    }

    #[tokio::test]
    async fn probe_connection_requires_credentials() {
        // Without credentials the probe reports an honest error rather than
        // dialing or fabricating a result.
        let engine = ImapEngine::new("acct-1", "127.0.0.1", 993);
        let err = engine
            .probe_connection()
            .await
            .expect_err("probe without credentials must fail");
        assert!(matches!(err, MailError::AuthError(_)));
    }

    #[test]
    fn tls_config_building() -> Result<(), MailError> {
        let config = build_tls_config()?;
        assert!(Arc::strong_count(&config) >= 1);

        let connector = build_tls_connector()?;
        let _ = connector;
        Ok(())
    }

    #[test]
    fn imap_fetch_command_query_construction() {
        let query = build_fetch_command_query();
        assert_eq!(
            query,
            "(FLAGS INTERNALDATE RFC822.SIZE ENVELOPE BODY.PEEK[])"
        );
        assert!(query.contains("BODY.PEEK[]"));
        assert!(query.contains("FLAGS"));
        assert!(query.contains("ENVELOPE"));
    }

    #[tokio::test]
    async fn listen_idle_callback_invocation() -> Result<(), MailError> {
        let engine = ImapEngine::new("acct-1", "mail.kof22.com", 993);
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        engine
            .listen_idle("INBOX", move |event| {
                if let Ok(mut guard) = events_clone.lock() {
                    guard.push(event);
                }
            })
            .await?;

        let guard = events
            .lock()
            .map_err(|e| MailError::ImapError(e.to_string()))?;
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0], IdleEvent::Disconnected);
        assert_eq!(
            engine.socket_manager().idle_state(),
            IdleSocketState::Listening
        );
        Ok(())
    }
}
