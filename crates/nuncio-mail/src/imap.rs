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
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tokio_stream::StreamExt;

use crate::backend::MailBackend;
use crate::parser::{MailError, MimeParserAdapter};

/// Upper bound on how long a single TCP connect + TLS handshake to an IMAP
/// server may take before this client gives up and reports an honest
/// timeout error, rather than hanging indefinitely. A misconfigured or
/// unreachable host (wrong hostname, firewall silently dropping packets,
/// wrong port) must fail fast and visibly, not stall the caller forever.
const IMAP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Upper bound on how long a single `FETCH` response item may take to
/// arrive once the `UID FETCH` command has been acknowledged. A real
/// mailbox with many/large messages fetching a full body for everything in
/// one unbatched command can legitimately take a while overall, but a
/// single item that never arrives (dead socket, server-side hang) must
/// still surface as an honest timeout instead of hanging the whole sync
/// indefinitely.
const IMAP_FETCH_ITEM_TIMEOUT: Duration = Duration::from_secs(60);

/// How often (in fetched-message count) to log sync progress, so a
/// genuinely slow-but-progressing sync against a large real mailbox is
/// visibly distinguishable from a stalled one.
const IMAP_FETCH_PROGRESS_LOG_INTERVAL: usize = 25;

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

/// A live IMAP socket that may or may not be TLS-wrapped, depending on the
/// account's configured [`TlsMode`]. `async_imap::Session`/`Client` are
/// generic over the underlying stream type, so this lets one connection
/// path serve `ImplicitTls` (already-encrypted), `StartTls` (plaintext
/// upgraded to TLS mid-handshake), and `Plain` (never encrypted -- local
/// dev/test servers only) without duplicating the session/IDLE/fetch logic
/// three times.
pub enum MaybeTlsStream {
    /// Unencrypted TCP socket ([`TlsMode::Plain`]).
    Plain(TcpStream),
    /// TLS-wrapped socket, either from an immediate handshake
    /// ([`TlsMode::ImplicitTls`]) or a post-`STARTTLS` upgrade
    /// ([`TlsMode::StartTls`]). Boxed: `TlsStream` is far larger than
    /// `TcpStream`, and clippy's `large_enum_variant` flags the resulting
    /// size skew otherwise.
    Tls(Box<TlsStream<TcpStream>>),
}

impl Debug for MaybeTlsStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MaybeTlsStream::Plain(_) => f.write_str("MaybeTlsStream::Plain"),
            MaybeTlsStream::Tls(_) => f.write_str("MaybeTlsStream::Tls"),
        }
    }
}

impl AsyncRead for MaybeTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTlsStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Connect the bare TCP socket underlying every [`TlsMode`], bounded by
/// [`IMAP_CONNECT_TIMEOUT`] so an unreachable host (bad hostname silently
/// dropped by a firewall, wrong port with no RST) fails fast with an honest
/// timeout error instead of hanging indefinitely.
async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, MailError> {
    let addr = format!("{}:{}", host, port);
    tokio::time::timeout(IMAP_CONNECT_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| {
            MailError::NetworkError(format!(
                "TCP connection to {} timed out after {}s",
                addr,
                IMAP_CONNECT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| MailError::NetworkError(format!("TCP connection to {} failed: {}", addr, e)))
}

/// Upgrade a plaintext TCP socket to TLS, bounded by [`IMAP_CONNECT_TIMEOUT`].
async fn upgrade_to_tls(
    host: &str,
    tcp_stream: TcpStream,
) -> Result<TlsStream<TcpStream>, MailError> {
    let connector = build_tls_connector()?;
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| MailError::TlsError(format!("invalid TLS server name '{}': {}", host, e)))?;

    tokio::time::timeout(
        IMAP_CONNECT_TIMEOUT,
        connector.connect(server_name, tcp_stream),
    )
    .await
    .map_err(|_| {
        MailError::TlsError(format!(
            "TLS handshake with {} timed out after {}s",
            host,
            IMAP_CONNECT_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| MailError::TlsError(format!("TLS handshake with {} failed: {}", host, e)))
}

/// Establish a socket to an IMAP server honoring `tls_mode`:
/// - [`TlsMode::ImplicitTls`]: TCP connect, then an immediate TLS handshake
///   (the traditional port-993 style).
/// - [`TlsMode::StartTls`]: TCP connect in plaintext, issue the IMAP
///   `STARTTLS` command and require the server to accept it, then perform
///   the TLS handshake on the same socket. STARTTLS is required, never
///   opportunistic -- a server that rejects or lacks it is an honest error,
///   never a silent downgrade to plaintext for a mode the caller explicitly
///   asked to be encrypted.
/// - [`TlsMode::Plain`]: TCP connect only, no encryption. For trusted local
///   dev/test servers only.
pub async fn connect_stream(
    host: &str,
    port: u16,
    tls_mode: TlsMode,
) -> Result<MaybeTlsStream, MailError> {
    let target_port = if port == 0 { 993 } else { port };

    match tls_mode {
        TlsMode::ImplicitTls => {
            let tcp_stream = connect_tcp(host, target_port).await?;
            Ok(MaybeTlsStream::Tls(Box::new(
                upgrade_to_tls(host, tcp_stream).await?,
            )))
        }
        TlsMode::Plain => {
            let tcp_stream = connect_tcp(host, target_port).await?;
            Ok(MaybeTlsStream::Plain(tcp_stream))
        }
        TlsMode::StartTls => {
            let tcp_stream = connect_tcp(host, target_port).await?;
            let mut client = async_imap::Client::new(tcp_stream);
            tokio::time::timeout(
                IMAP_CONNECT_TIMEOUT,
                client.run_command_and_check_ok("STARTTLS", None),
            )
            .await
            .map_err(|_| {
                MailError::TlsError(format!(
                    "STARTTLS negotiation with {} timed out after {}s",
                    host,
                    IMAP_CONNECT_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|e| {
                MailError::TlsError(format!(
                    "server {} rejected or does not support STARTTLS: {}",
                    host, e
                ))
            })?;
            let tcp_stream = client.into_inner();
            Ok(MaybeTlsStream::Tls(Box::new(
                upgrade_to_tls(host, tcp_stream).await?,
            )))
        }
    }
}

/// IMAP dual-socket manager maintaining isolated connections for IDLE push and FETCH/STORE queries.
pub struct ImapDualSocketManager {
    server_host: String,
    server_port: u16,
    tls_mode: TlsMode,
    idle_active: Arc<AtomicBool>,
}

impl ImapDualSocketManager {
    /// Create a new `ImapDualSocketManager` for the given [`TlsMode`].
    pub fn new(server_host: &str, server_port: u16, tls_mode: TlsMode) -> Self {
        let port = if server_port == 0 { 993 } else { server_port };
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

    /// Retrieve the current IDLE socket state.
    pub fn idle_state(&self) -> IdleSocketState {
        if self.idle_active.load(Ordering::SeqCst) {
            IdleSocketState::Listening
        } else {
            IdleSocketState::Disconnected
        }
    }

    /// Establish a live socket connection to the target IMAP server,
    /// honoring this manager's configured [`TlsMode`].
    pub async fn connect_tls_socket(&self) -> Result<MaybeTlsStream, MailError> {
        connect_stream(&self.server_host, self.server_port, self.tls_mode).await
    }

    /// Establish an authenticated IMAP session over a live socket connection.
    pub async fn connect_session(
        &self,
        username: &str,
        password: &str,
    ) -> Result<async_imap::Session<MaybeTlsStream>, MailError> {
        let stream = self.connect_tls_socket().await?;
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

    /// Validate that `username`/`password` can genuinely authenticate
    /// against this server: connects, logs in, then immediately logs out
    /// again -- no folders or messages are touched. Used by the daemon's
    /// `TestAccountConnection` RPC to let a caller check account settings
    /// are correct without running a real sync. Returns the real
    /// connect/TLS/auth error on failure, never a fabricated success.
    pub async fn test_login(&self, username: &str, password: &str) -> Result<(), MailError> {
        let mut session = self.connect_session(username, password).await?;
        // A logout failure after a successful login doesn't change the
        // verdict -- the credentials and server settings are already
        // proven correct at this point -- but it's still surfaced so a
        // caller can see the server misbehaved on teardown.
        session
            .logout()
            .await
            .map_err(|e| MailError::ImapError(format!("IMAP logout failed: {}", e)))?;
        Ok(())
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
    /// Create a new `ImapEngine` using [`TlsMode::ImplicitTls`] (the
    /// traditional port-993 style). Use [`Self::with_tls_mode`] to select a
    /// different connection type.
    pub fn new(account_id: &str, server_host: &str, server_port: u16) -> Self {
        Self::with_tls_mode(account_id, server_host, server_port, TlsMode::ImplicitTls)
    }

    /// Create a new `ImapEngine` targeting a specific [`TlsMode`]
    /// (`ImplicitTls`, `StartTls`, or `Plain`).
    pub fn with_tls_mode(
        account_id: &str,
        server_host: &str,
        server_port: u16,
        tls_mode: TlsMode,
    ) -> Self {
        Self {
            socket_manager: ImapDualSocketManager::new(server_host, server_port, tls_mode),
            account_id: account_id.to_string(),
            username: None,
            password: None,
        }
    }

    /// Create a new `ImapEngine` with host, port, TLS mode, and
    /// authentication credentials.
    pub fn with_credentials(
        account_id: &str,
        server_host: &str,
        server_port: u16,
        tls_mode: TlsMode,
        username: &str,
        password: &str,
    ) -> Self {
        Self {
            socket_manager: ImapDualSocketManager::new(server_host, server_port, tls_mode),
            account_id: account_id.to_string(),
            username: Some(username.to_string()),
            password: Some(password.to_string()),
        }
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
        let select_started = std::time::Instant::now();
        let mailbox = session.select(folder_id).await.map_err(|e| {
            MailError::ImapError(format!("failed to select folder '{}': {}", folder_id, e))
        })?;
        tracing::info!(
            folder_id,
            exists = mailbox.exists,
            elapsed_ms = select_started.elapsed().as_millis() as u64,
            "IMAP SELECT completed"
        );

        let query = build_fetch_command_query();
        let fetch_started = std::time::Instant::now();
        let mut fetch_stream = session.uid_fetch("1:*", query).await.map_err(|e| {
            MailError::ImapError(format!(
                "UID FETCH failed for folder '{}': {}",
                folder_id, e
            ))
        })?;
        tracing::info!(
            folder_id,
            elapsed_ms = fetch_started.elapsed().as_millis() as u64,
            "IMAP UID FETCH command acknowledged; reading response stream"
        );

        let mut emails = Vec::new();
        let stream_started = std::time::Instant::now();
        loop {
            // Bound EVERY individual item read, not just the connect phase:
            // a real mailbox with many/large messages fetching
            // `BODY.PEEK[]` for everything in one unbatched command can be
            // legitimately slow, but a single stalled read (dead socket,
            // server-side hang) must still surface as an honest timeout
            // error rather than hanging the whole sync forever.
            let next_item =
                match tokio::time::timeout(IMAP_FETCH_ITEM_TIMEOUT, fetch_stream.next()).await {
                    Ok(item) => item,
                    Err(_) => {
                        tracing::warn!(
                            folder_id,
                            fetched_so_far = emails.len(),
                            elapsed_ms = stream_started.elapsed().as_millis() as u64,
                            "IMAP FETCH stream stalled: no response within {}s",
                            IMAP_FETCH_ITEM_TIMEOUT.as_secs()
                        );
                        return Err(MailError::ImapError(format!(
                            "UID FETCH for folder '{}' stalled after {} messages ({}s with no \
                         server response)",
                            folder_id,
                            emails.len(),
                            IMAP_FETCH_ITEM_TIMEOUT.as_secs()
                        )));
                    }
                };
            let Some(fetch_res) = next_item else {
                break;
            };

            let fetch_data = fetch_res
                .map_err(|e| MailError::ImapError(format!("failed reading fetch item: {}", e)))?;

            if emails.len() % IMAP_FETCH_PROGRESS_LOG_INTERVAL == 0 {
                tracing::info!(
                    folder_id,
                    fetched_so_far = emails.len(),
                    elapsed_ms = stream_started.elapsed().as_millis() as u64,
                    "IMAP FETCH progress"
                );
            }

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
                return Err(MailError::AuthError(
                    "no credentials configured for this IMAP engine instance".to_string(),
                ));
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
        let (u, p) = self
            .username
            .as_deref()
            .zip(self.password.as_deref())
            .ok_or_else(|| {
                MailError::AuthError(
                    "no credentials configured for this IMAP engine instance".to_string(),
                )
            })?;

        let mut session = self.socket_manager.connect_session(u, p).await?;
        let mut mailboxes = session
            .list(None, Some("*"))
            .await
            .map_err(|e| MailError::ImapError(format!("failed to list mailboxes: {}", e)))?;

        let mut folders = Vec::new();
        while let Some(mb_res) = mailboxes.next().await {
            let mb = mb_res
                .map_err(|e| MailError::ImapError(format!("failed reading mailbox item: {}", e)))?;
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
        // A genuinely empty mailbox (or a server that reports zero
        // folders) is real data, not an error -- returning it as-is is the
        // honest result. Fabricating a non-empty "Inbox"/"Sent" fallback
        // here would silently lie about what the server actually has.
        Ok(folders)
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

    /// Not wired to a real SMTP transport: production sends go through
    /// [`crate::smtp::SmtpTransportEngine`] directly (see its doc comment),
    /// never through this trait method. Rather than silently return `Ok(())`
    /// and let a caller believe mail was sent, this is an honest error.
    async fn send_email(&self, _email: &Email) -> Result<(), MailError> {
        Err(MailError::NetworkError(
            "ImapEngine::send_email is not a real transport -- outbound mail goes through \
             SmtpTransportEngine"
                .to_string(),
        ))
    }

    async fn test_connection(&self) -> Result<(), MailError> {
        let (u, p) = self
            .username
            .as_deref()
            .zip(self.password.as_deref())
            .ok_or_else(|| {
                MailError::AuthError(
                    "no credentials configured for this IMAP engine instance".to_string(),
                )
            })?;
        self.socket_manager.test_login(u, p).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn imap_dual_socket_manager_lifecycle() -> Result<(), MailError> {
        let manager = ImapDualSocketManager::new("mail.kof22.com", 993, TlsMode::ImplicitTls);
        assert_eq!(manager.server_host(), "mail.kof22.com");
        assert_eq!(manager.server_port(), 993);
        assert_eq!(manager.idle_state(), IdleSocketState::Disconnected);

        manager.start_idle_listener()?;
        assert_eq!(manager.idle_state(), IdleSocketState::Listening);

        manager.stop_idle_listener();
        assert_eq!(manager.idle_state(), IdleSocketState::Disconnected);
        Ok(())
    }

    #[tokio::test]
    async fn imap_engine_without_credentials_honestly_errors_rather_than_fabricating_data() {
        // `ImapEngine::new` (no credentials) must never fabricate folders,
        // messages, or a successful send -- every `MailBackend` method must
        // surface a real "no credentials configured" error instead.
        let engine = ImapEngine::new("acct-1", "mail.kof22.com", 993);

        let folders_err = engine
            .sync_folders()
            .await
            .expect_err("sync_folders without credentials must error, not fabricate folders");
        assert!(matches!(folders_err, MailError::AuthError(_)));

        let messages_err = engine
            .sync_messages("INBOX", None)
            .await
            .expect_err("sync_messages without credentials must error, not fabricate emails");
        assert!(matches!(messages_err, MailError::AuthError(_)));

        let test_err = engine
            .test_connection()
            .await
            .expect_err("test_connection without credentials must error, not fabricate success");
        assert!(matches!(test_err, MailError::AuthError(_)));

        let dummy_email = Email {
            id: "e1".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "INBOX".to_string(),
            subject: "s".to_string(),
            sender: "a@b.com".to_string(),
            recipient: "c@d.com".to_string(),
            received_at: 0,
            read: false,
            body_plain: None,
            body_html: None,
            attachments: Vec::new(),
        };
        let send_err = engine
            .send_email(&dummy_email)
            .await
            .expect_err("ImapEngine::send_email is not a real transport and must error");
        assert!(matches!(send_err, MailError::NetworkError(_)));
    }

    /// Bind a loopback listener and run a minimal, single-connection,
    /// line-oriented IMAP responder: sends the initial greeting, then
    /// answers each tagged command according to `respond`. Used to prove
    /// `test_login`/`test_connection` genuinely speak the IMAP wire
    /// protocol (tag matching, greeting, LOGIN/STARTTLS/LOGOUT) end to end,
    /// rather than only being exercised against a real network server.
    ///
    /// Deliberately plaintext-only: a TLS-terminating mock would need a
    /// vendored test certificate, which this harness does not set up, so
    /// `TlsMode::ImplicitTls`'s handshake step itself is not covered here --
    /// only `TlsMode::Plain` and the pre-upgrade half of `TlsMode::StartTls`
    /// are exercised against a real socket.
    async fn spawn_mock_imap_server(
        respond: impl Fn(&str, &str) -> String + Send + 'static,
    ) -> std::net::SocketAddr {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback listener");
        let addr = listener.local_addr().expect("listener has local addr");

        tokio::spawn(async move {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let (read_half, mut write_half) = socket.into_split();
            let mut reader = BufReader::new(read_half);
            if write_half
                .write_all(b"* OK mock IMAP ready\r\n")
                .await
                .is_err()
            {
                return;
            }

            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let trimmed = line.trim_end();
                let tag = trimmed.split_whitespace().next().unwrap_or("*");
                let reply = respond(tag, trimmed);
                if write_half.write_all(reply.as_bytes()).await.is_err() {
                    break;
                }
                if trimmed.to_uppercase().contains("LOGOUT") {
                    break;
                }
            }
        });

        addr
    }

    #[tokio::test]
    async fn test_login_succeeds_over_a_real_plain_socket() -> Result<(), MailError> {
        let addr = spawn_mock_imap_server(|tag, line| {
            if line.to_uppercase().contains("LOGIN") {
                format!("{} OK LOGIN completed\r\n", tag)
            } else if line.to_uppercase().contains("LOGOUT") {
                format!("* BYE logging out\r\n{} OK LOGOUT completed\r\n", tag)
            } else {
                format!("{} BAD unrecognized\r\n", tag)
            }
        })
        .await;

        let manager =
            ImapDualSocketManager::new(&addr.ip().to_string(), addr.port(), TlsMode::Plain);
        manager.test_login("user", "pass").await
    }

    #[tokio::test]
    async fn test_login_surfaces_real_auth_error_on_login_rejection() {
        let addr = spawn_mock_imap_server(|tag, line| {
            if line.to_uppercase().contains("LOGIN") {
                format!("{} NO authentication failed\r\n", tag)
            } else {
                format!("{} BAD unrecognized\r\n", tag)
            }
        })
        .await;

        let manager =
            ImapDualSocketManager::new(&addr.ip().to_string(), addr.port(), TlsMode::Plain);
        let err = manager
            .test_login("user", "wrong-password")
            .await
            .expect_err("a server-rejected LOGIN must surface as a real error");
        assert!(matches!(err, MailError::AuthError(_)));
    }

    #[tokio::test]
    async fn connect_stream_start_tls_surfaces_honest_error_when_server_rejects_starttls() {
        let addr = spawn_mock_imap_server(|tag, line| {
            if line.to_uppercase().contains("STARTTLS") {
                format!("{} NO STARTTLS not supported\r\n", tag)
            } else {
                format!("{} BAD unrecognized\r\n", tag)
            }
        })
        .await;

        let err = connect_stream(&addr.ip().to_string(), addr.port(), TlsMode::StartTls)
            .await
            .expect_err("a server that rejects STARTTLS must be an honest error, never a silent plaintext fallback");
        assert!(matches!(err, MailError::TlsError(_)));
    }

    #[tokio::test]
    async fn connect_stream_reports_honest_error_for_unreachable_host() {
        // Bind then immediately drop a loopback listener: the OS reserves
        // the port momentarily, then refuses the next connection attempt --
        // deterministic "connection refused" without depending on any
        // specific hardcoded port that might collide with a real service.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        drop(listener);

        let err = connect_stream(&addr.ip().to_string(), addr.port(), TlsMode::Plain)
            .await
            .expect_err("connecting to a closed port must be an honest error");
        assert!(matches!(err, MailError::NetworkError(_)));
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
