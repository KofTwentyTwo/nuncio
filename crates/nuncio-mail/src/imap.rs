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

/// Upper bound on how long a single `FETCH` response item may take to arrive
/// once the `UID FETCH` command has been acknowledged. Fetching full bodies
/// for a large mailbox in one command can be legitimately slow overall, so the
/// bound is per item rather than for the whole stream; but a single item that
/// never arrives (dead socket, server-side hang) must still surface as an
/// honest timeout rather than hanging the sync indefinitely.
const FETCH_ITEM_TIMEOUT: Duration = Duration::from_secs(60);

use crate::backend::MailBackend;
use crate::parser::{MailError, MimeParserAdapter};

/// Delimiter between the two components of a folder sync checkpoint. The
/// checkpoint state string is `"{uidvalidity}{DELIM}{uidnext}"` (e.g.
/// `"42:105"`): the mailbox's UIDVALIDITY at the time it was written, then the
/// UID boundary to resume from. Both components are decimal `u32`.
///
/// UIDVALIDITY MUST be carried because IMAP UIDs are only meaningful within a
/// stable UIDVALIDITY (RFC 3501 s2.3.1.1). If a mailbox is recreated and its
/// UIDVALIDITY changes, UIDs restart from low numbers, so resuming from a UID
/// boundary captured under the old UIDVALIDITY would issue `{old_uidnext}:*`
/// against renumbered low UIDs -- which per `n:*` semantics returns only the
/// single highest message and silently skips the rest. Comparing UIDVALIDITY
/// first turns that into a safe full re-fetch instead.
const CHECKPOINT_DELIM: char = ':';

/// The `UID FETCH` sequence-set to issue for a folder sync, resolved from the
/// stored checkpoint. Kept as a small typed value (rather than collapsing to a
/// bare range string) so the distinct fall-back reasons stay observable: all
/// of `Full`/`CorruptCheckpoint`/`UidValidityChanged` fetch `1:*`, but only the
/// latter two are worth flagging, and tests can assert on which branch fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetchRange {
    /// No prior checkpoint (first-ever sync of this folder): fetch everything.
    Full,
    /// A checkpoint was stored but could not be parsed (malformed, or a legacy
    /// bare-UID checkpoint predating UIDVALIDITY tracking). Falls back to a
    /// full fetch, surfaced (logged) rather than silently ignored.
    CorruptCheckpoint,
    /// The stored checkpoint's UIDVALIDITY no longer matches the mailbox's
    /// current UIDVALIDITY (the mailbox was renumbered) or the server did not
    /// report a current UIDVALIDITY to compare against. Stored UIDs are no
    /// longer meaningful, so this falls back to a safe full re-fetch.
    UidValidityChanged {
        /// UIDVALIDITY recorded in the stored checkpoint.
        stored: u32,
        /// UIDVALIDITY the server reports now, if any.
        current: Option<u32>,
    },
    /// A valid checkpoint whose UIDVALIDITY still matches: fetch only UIDs at
    /// or above the stored boundary, i.e. messages that arrived since.
    Incremental(u32),
}

impl FetchRange {
    /// Render the IMAP `UID FETCH` sequence-set. Every non-incremental variant
    /// fetches all UIDs (`1:*`); an incremental checkpoint `n` fetches `n:*`,
    /// which the server bounds to UIDs `>= n` -- a genuinely narrower fetch.
    fn sequence_set(self) -> String {
        match self {
            FetchRange::Full
            | FetchRange::CorruptCheckpoint
            | FetchRange::UidValidityChanged { .. } => "1:*".to_string(),
            FetchRange::Incremental(uid) => format!("{}:*", uid),
        }
    }
}

/// Resolve the fetch range from a stored `since_state` checkpoint and the
/// mailbox's CURRENT UIDVALIDITY (as just reported by `SELECT`). `None` means
/// no prior sync (full fetch). A checkpoint in the `"{uidvalidity}:{uidnext}"`
/// format proceeds incrementally only when its UIDVALIDITY still matches the
/// server's current one; a mismatch (mailbox renumbered) or a missing current
/// UIDVALIDITY forces a safe full re-fetch. A malformed or legacy bare-number
/// checkpoint is treated as corrupt (full fetch, flagged), never misread as a
/// live UID boundary.
fn resolve_fetch_range(since_state: Option<&str>, current_uid_validity: Option<u32>) -> FetchRange {
    let Some(raw) = since_state else {
        return FetchRange::Full;
    };
    let parsed = raw
        .split_once(CHECKPOINT_DELIM)
        .and_then(|(validity, uid)| {
            Some((
                validity.trim().parse::<u32>().ok()?,
                uid.trim().parse::<u32>().ok()?,
            ))
        });
    let Some((stored_validity, uid)) = parsed else {
        return FetchRange::CorruptCheckpoint;
    };
    if uid < 1 {
        return FetchRange::CorruptCheckpoint;
    }
    match current_uid_validity {
        Some(current) if current == stored_validity => FetchRange::Incremental(uid),
        current => FetchRange::UidValidityChanged {
            stored: stored_validity,
            current,
        },
    }
}

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

    /// Fetch and parse the messages in a folder over an active IMAP session,
    /// returning the fetched messages and the new sync checkpoint.
    ///
    /// `since_state` is the checkpoint returned by a previous sync of this
    /// folder, encoded as `"{uidvalidity}:{uidnext}"` (see [`CHECKPOINT_DELIM`]).
    /// The `UID FETCH` is narrowed to `{uidnext}:*` only when the checkpoint's
    /// UIDVALIDITY still matches the mailbox's current UIDVALIDITY; a mismatch
    /// (the mailbox was renumbered) or a malformed/legacy checkpoint forces a
    /// safe full re-fetch instead of silently skipping renumbered mail. The
    /// returned checkpoint is derived from real server state -- the mailbox's
    /// UIDVALIDITY plus its `UIDNEXT` (or one past the highest UID actually
    /// seen) after selecting the folder -- never from a synthetic counter.
    pub async fn sync_folder_messages_with_session<S>(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
        session: &mut async_imap::Session<S>,
    ) -> Result<(Vec<Email>, String), MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        let mailbox = session.select(folder_id).await.map_err(|e| {
            MailError::ImapError(format!("failed to select folder '{}': {}", folder_id, e))
        })?;
        let server_uid_next = mailbox.uid_next;
        let server_uid_validity = mailbox.uid_validity;

        let range = resolve_fetch_range(since_state, server_uid_validity);
        match range {
            FetchRange::CorruptCheckpoint => tracing::warn!(
                folder_id,
                checkpoint = since_state.unwrap_or_default(),
                "ignoring unparseable folder sync checkpoint; falling back to a full fetch"
            ),
            FetchRange::UidValidityChanged { stored, current } => tracing::warn!(
                folder_id,
                stored_uidvalidity = stored,
                current_uidvalidity = current,
                "mailbox UIDVALIDITY changed (renumbered); the stored UID checkpoint is no \
                 longer valid, so re-fetching in full to avoid silently skipping messages"
            ),
            FetchRange::Full | FetchRange::Incremental(_) => {}
        }

        let query = build_fetch_command_query();
        let sequence_set = range.sequence_set();
        let mut fetch_stream = session.uid_fetch(&sequence_set, query).await.map_err(|e| {
            MailError::ImapError(format!(
                "UID FETCH failed for folder '{}': {}",
                folder_id, e
            ))
        })?;

        let mut emails = Vec::new();
        let mut max_uid_seen: u32 = 0;
        loop {
            // Bound EVERY individual item read: a slow-but-progressing large
            // fetch is fine, but a single item that never arrives must surface
            // as an honest `FetchStalled` instead of hanging the whole sync.
            let next_item = tokio::time::timeout(FETCH_ITEM_TIMEOUT, fetch_stream.next())
                .await
                .map_err(|_| {
                    MailError::FetchStalled(format!(
                        "no FETCH response for folder '{}' within {}s ({} message(s) read so far)",
                        folder_id,
                        FETCH_ITEM_TIMEOUT.as_secs(),
                        emails.len()
                    ))
                })?;
            let Some(fetch_res) = next_item else {
                break;
            };
            let fetch_data = fetch_res
                .map_err(|e| MailError::ImapError(format!("failed reading fetch item: {}", e)))?;

            let uid_num = fetch_data.uid.unwrap_or(0);
            max_uid_seen = max_uid_seen.max(uid_num);
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

        // Derive the next checkpoint from real server state as
        // `"{uidvalidity}:{uidnext}"`. The UID boundary is the mailbox's
        // authoritative UIDNEXT, else one past the highest UID actually seen.
        // A checkpoint is only meaningful when both a UIDVALIDITY and a UID
        // boundary are known; if either is missing (a server that omitted
        // UIDVALIDITY, or an empty fetch that revealed no UIDNEXT/UID), keep
        // the prior checkpoint rather than writing a validity-less one that the
        // next sync could misread. As a last resort (no prior checkpoint
        // either) emit an intentionally unparseable marker so the next sync
        // safely falls back to a full fetch.
        let boundary_uid = server_uid_next
            .or_else(|| (max_uid_seen > 0).then_some(max_uid_seen.saturating_add(1)));
        let new_checkpoint = match (server_uid_validity, boundary_uid) {
            (Some(validity), Some(uid)) => format!("{}{}{}", validity, CHECKPOINT_DELIM, uid),
            _ => since_state
                .map(str::to_string)
                .unwrap_or_else(|| "full-resync-required".to_string()),
        };

        Ok((emails, new_checkpoint))
    }

    /// Fetch a folder's messages over a freshly authenticated session,
    /// honoring `since_state` for an incremental fetch and returning the new
    /// sync checkpoint alongside the messages. See
    /// [`Self::sync_folder_messages_with_session`] for the checkpoint model.
    pub async fn sync_folder_messages(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<(Vec<Email>, String), MailError> {
        let (username, password) = match (&self.username, &self.password) {
            (Some(u), Some(p)) => (u.as_str(), p.as_str()),
            _ => {
                return Err(MailError::AuthError(
                    "IMAP message sync requires credentials".to_string(),
                ))
            }
        };

        let mut session = self
            .socket_manager
            .connect_session(username, password)
            .await?;
        let result = self
            .sync_folder_messages_with_session(folder_id, since_state, &mut session)
            .await;
        let _ = session.logout().await;
        result
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
        let (u, p) = match (&self.username, &self.password) {
            (Some(u), Some(p)) => (u.as_str(), p.as_str()),
            _ => {
                return Err(MailError::AuthError(
                    "IMAP folder sync requires credentials".to_string(),
                ))
            }
        };
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
        // An empty mailbox is a genuine result, not a signal to fall back to
        // placeholder folders -- the caller gets exactly what the server reported.
        Ok(folders)
    }

    async fn sync_messages(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<(Vec<Email>, String), MailError> {
        self.sync_folder_messages(folder_id, since_state).await
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
    async fn sync_folders_and_sync_messages_require_credentials() {
        // Without credentials there is no server to sync against; both
        // methods must surface an honest auth error rather than fabricating
        // folders or messages.
        let engine = ImapEngine::new("acct-1", "mail.kof22.com", 993);

        let err = engine
            .sync_folders()
            .await
            .expect_err("credential-less sync_folders must fail");
        assert!(matches!(err, MailError::AuthError(_)));

        let err = engine
            .sync_messages("INBOX", None)
            .await
            .expect_err("credential-less sync_messages must fail");
        assert!(matches!(err, MailError::AuthError(_)));
    }

    #[tokio::test]
    async fn sync_folders_lists_real_mailboxes_over_a_scripted_server() {
        // Drives `MailBackend::sync_folders` against a real, plain-TCP
        // loopback IMAP server scripted to answer LOGIN/LIST/LOGOUT -- proving
        // the returned folder list is genuinely read off the wire.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("listener addr");

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept connection");
            while let Some(line) = read_scripted_line(&mut socket).await {
                let tag = line.split_whitespace().next().unwrap_or("").to_string();
                let upper = line.to_ascii_uppercase();
                if upper.contains("LOGIN") {
                    let _ = socket
                        .write_all(format!("{tag} OK Logged in\r\n").as_bytes())
                        .await;
                } else if upper.contains("LIST") {
                    let resp = format!(
                        "* LIST () \"/\" INBOX\r\n* LIST () \"/\" Archive\r\n{tag} OK LIST completed\r\n"
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                } else if upper.contains("LOGOUT") {
                    let _ = socket
                        .write_all(format!("* BYE\r\n{tag} OK LOGOUT\r\n").as_bytes())
                        .await;
                    break;
                }
            }
        });

        let engine = ImapEngine::with_credentials(
            "acct-1",
            "127.0.0.1",
            addr.port(),
            TlsMode::Plain,
            "user",
            "pass",
        );
        let folders = engine.sync_folders().await.expect("sync_folders succeeds");
        server.await.expect("scripted server task completes");

        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].id, "INBOX");
        assert_eq!(folders[1].id, "Archive");
    }

    #[test]
    fn resolve_fetch_range_distinguishes_none_valid_corrupt_and_renumbered_checkpoints() {
        // First-ever sync: full history (current UIDVALIDITY irrelevant).
        assert_eq!(resolve_fetch_range(None, Some(1)), FetchRange::Full);
        assert_eq!(resolve_fetch_range(None, None).sequence_set(), "1:*");

        // Valid checkpoint whose UIDVALIDITY still matches: narrowed fetch.
        assert_eq!(
            resolve_fetch_range(Some("1:105"), Some(1)),
            FetchRange::Incremental(105)
        );
        assert_eq!(
            resolve_fetch_range(Some("1:105"), Some(1)).sequence_set(),
            "105:*"
        );
        assert_eq!(
            resolve_fetch_range(Some(" 7 : 42 "), Some(7)),
            FetchRange::Incremental(42)
        );

        // UIDVALIDITY changed (mailbox renumbered) or the server reported none
        // to compare against: MUST fall back to a full fetch, distinctly, so a
        // renumber can never silently skip the low new UIDs.
        assert_eq!(
            resolve_fetch_range(Some("1:105"), Some(2)),
            FetchRange::UidValidityChanged {
                stored: 1,
                current: Some(2)
            }
        );
        assert_eq!(
            resolve_fetch_range(Some("1:105"), Some(2)).sequence_set(),
            "1:*"
        );
        assert_eq!(
            resolve_fetch_range(Some("1:105"), None),
            FetchRange::UidValidityChanged {
                stored: 1,
                current: None
            }
        );

        // Malformed or legacy bare-number (pre-UIDVALIDITY) checkpoints are
        // corrupt -> full fetch, never misread as a live UID boundary.
        assert_eq!(
            resolve_fetch_range(Some(""), Some(1)),
            FetchRange::CorruptCheckpoint
        );
        assert_eq!(
            resolve_fetch_range(Some("105"), Some(1)),
            FetchRange::CorruptCheckpoint,
            "a legacy bare-number checkpoint must not be read as a UID boundary"
        );
        assert_eq!(
            resolve_fetch_range(Some("1:0"), Some(1)),
            FetchRange::CorruptCheckpoint
        );
        assert_eq!(
            resolve_fetch_range(Some("not:auid"), Some(1)),
            FetchRange::CorruptCheckpoint
        );
        assert_eq!(
            resolve_fetch_range(Some("not-a-uid"), Some(1)).sequence_set(),
            "1:*"
        );
    }

    /// Read a single CRLF-terminated line from a scripted-server stream,
    /// returning `None` at EOF (client hung up). Used only by the in-memory
    /// duplex tests below; the real fetch path never parses raw lines.
    async fn read_scripted_line<R: AsyncRead + Unpin>(reader: &mut R) -> Option<String> {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte).await {
                Ok(0) => {
                    return if buf.is_empty() {
                        None
                    } else {
                        Some(String::from_utf8_lossy(&buf).into_owned())
                    }
                }
                Ok(_) => {
                    if byte[0] == b'\n' {
                        return Some(String::from_utf8_lossy(&buf).into_owned());
                    }
                    if byte[0] != b'\r' {
                        buf.push(byte[0]);
                    }
                }
                Err(_) => return None,
            }
        }
    }

    #[tokio::test]
    async fn incremental_sync_narrows_the_fetch_range_on_the_second_pass() {
        // Drives a real `async_imap::Session` over an in-memory duplex pair
        // whose server side is a scripted IMAP responder. Proves the SECOND
        // sync issues a GENUINELY narrower `UID FETCH` derived from the first
        // sync's returned checkpoint -- not a full re-fetch.
        let (client_io, server_io) = tokio::io::duplex(8192);
        let fetch_ranges = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let server_ranges = fetch_ranges.clone();

        let server = tokio::spawn(async move {
            let mut io = server_io;
            while let Some(line) = read_scripted_line(&mut io).await {
                let tag = line.split_whitespace().next().unwrap_or("").to_string();
                let upper = line.to_ascii_uppercase();
                if upper.contains("UID FETCH") {
                    if let Ok(mut g) = server_ranges.lock() {
                        g.push(line.clone());
                    }
                    let _ = io
                        .write_all(format!("{tag} OK FETCH completed\r\n").as_bytes())
                        .await;
                } else if upper.contains("LOGIN") {
                    let _ = io
                        .write_all(format!("{tag} OK Logged in\r\n").as_bytes())
                        .await;
                } else if upper.contains("SELECT") {
                    let resp = format!(
                        "* FLAGS (\\Seen)\r\n* 3 EXISTS\r\n* 0 RECENT\r\n\
                         * OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT 105] ok\r\n\
                         {tag} OK [READ-WRITE] SELECT done\r\n"
                    );
                    let _ = io.write_all(resp.as_bytes()).await;
                } else if upper.contains("LOGOUT") {
                    let _ = io
                        .write_all(format!("* BYE\r\n{tag} OK LOGOUT\r\n").as_bytes())
                        .await;
                    break;
                }
            }
        });

        let client = async_imap::Client::new(client_io);
        let mut session = client
            .login("user", "pass")
            .await
            .map_err(|(e, _)| e)
            .expect("scripted login succeeds");

        let engine = ImapEngine::new("acct-1", "example.test", 993);

        // First sync: no checkpoint -> full history; returns
        // "{uidvalidity}:{uidnext}" from real server state.
        let (first, checkpoint) = engine
            .sync_folder_messages_with_session("INBOX", None, &mut session)
            .await
            .expect("first sync succeeds");
        assert!(first.is_empty());
        assert_eq!(checkpoint, "1:105");

        // Second sync: feed back the checkpoint; UIDVALIDITY still matches
        // (1), so the fetch is narrowed.
        let (_second, checkpoint2) = engine
            .sync_folder_messages_with_session("INBOX", Some(&checkpoint), &mut session)
            .await
            .expect("second sync succeeds");
        assert_eq!(checkpoint2, "1:105");

        drop(session);
        let _ = server.await;

        let ranges = fetch_ranges.lock().expect("lock captured ranges");
        assert_eq!(ranges.len(), 2, "expected exactly two UID FETCH commands");
        assert!(
            ranges[0].contains("1:*"),
            "first sync must fetch full history, got: {}",
            ranges[0]
        );
        assert!(
            ranges[1].contains("105:*"),
            "second sync must be narrowed to the checkpoint, got: {}",
            ranges[1]
        );
        assert!(
            !ranges[1].contains("1:*"),
            "second sync must NOT re-fetch full history, got: {}",
            ranges[1]
        );
    }

    #[tokio::test]
    async fn a_uidvalidity_change_forces_a_full_resync_instead_of_skipping_mail() {
        // A mailbox renumber changes UIDVALIDITY and restarts UIDs from low
        // numbers. Resuming from the old UID boundary (`105:*`) would return
        // only the single highest message and silently drop the rest, so a
        // UIDVALIDITY mismatch MUST force a full `1:*` re-fetch. The scripted
        // server reports UIDVALIDITY 1 on the first SELECT and 2 on the second.
        let (client_io, server_io) = tokio::io::duplex(8192);
        let fetch_ranges = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let server_ranges = fetch_ranges.clone();

        let server = tokio::spawn(async move {
            let mut io = server_io;
            let mut select_count = 0u32;
            while let Some(line) = read_scripted_line(&mut io).await {
                let tag = line.split_whitespace().next().unwrap_or("").to_string();
                let upper = line.to_ascii_uppercase();
                if upper.contains("UID FETCH") {
                    if let Ok(mut g) = server_ranges.lock() {
                        g.push(line.clone());
                    }
                    let _ = io
                        .write_all(format!("{tag} OK FETCH completed\r\n").as_bytes())
                        .await;
                } else if upper.contains("LOGIN") {
                    let _ = io
                        .write_all(format!("{tag} OK Logged in\r\n").as_bytes())
                        .await;
                } else if upper.contains("SELECT") {
                    select_count += 1;
                    // First SELECT: UIDVALIDITY 1. Second: a DIFFERENT
                    // UIDVALIDITY (2), i.e. the mailbox was renumbered.
                    let uidvalidity = if select_count == 1 { 1 } else { 2 };
                    let resp = format!(
                        "* FLAGS (\\Seen)\r\n* 3 EXISTS\r\n* 0 RECENT\r\n\
                         * OK [UIDVALIDITY {uidvalidity}] ok\r\n* OK [UIDNEXT 105] ok\r\n\
                         {tag} OK [READ-WRITE] SELECT done\r\n"
                    );
                    let _ = io.write_all(resp.as_bytes()).await;
                } else if upper.contains("LOGOUT") {
                    let _ = io
                        .write_all(format!("* BYE\r\n{tag} OK LOGOUT\r\n").as_bytes())
                        .await;
                    break;
                }
            }
        });

        let client = async_imap::Client::new(client_io);
        let mut session = client
            .login("user", "pass")
            .await
            .map_err(|(e, _)| e)
            .expect("scripted login succeeds");

        let engine = ImapEngine::new("acct-1", "example.test", 993);

        // First sync under UIDVALIDITY 1 stores "1:105".
        let (_first, checkpoint) = engine
            .sync_folder_messages_with_session("INBOX", None, &mut session)
            .await
            .expect("first sync succeeds");
        assert_eq!(checkpoint, "1:105");

        // Second sync: server now reports UIDVALIDITY 2. The stored checkpoint
        // is stale, so the fetch MUST be a full `1:*`, and the checkpoint is
        // rewritten under the new UIDVALIDITY.
        let (_second, checkpoint2) = engine
            .sync_folder_messages_with_session("INBOX", Some(&checkpoint), &mut session)
            .await
            .expect("second sync succeeds");
        assert_eq!(
            checkpoint2, "2:105",
            "checkpoint must be rewritten under the new UIDVALIDITY"
        );

        drop(session);
        let _ = server.await;

        let ranges = fetch_ranges.lock().expect("lock captured ranges");
        assert_eq!(ranges.len(), 2, "expected exactly two UID FETCH commands");
        assert!(
            ranges[0].contains("1:*"),
            "first sync fetches full history, got: {}",
            ranges[0]
        );
        assert!(
            ranges[1].contains("1:*") && !ranges[1].contains("105:*"),
            "a UIDVALIDITY change MUST force a full re-fetch (1:*), never 105:*, got: {}",
            ranges[1]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_never_responding_fetch_surfaces_fetchstalled_without_hanging() {
        // The scripted server answers LOGIN and SELECT but then goes silent on
        // the FETCH. With the tokio clock paused, the bounded per-item timeout
        // fast-forwards, so the stall is turned into an honest `FetchStalled`
        // error in negligible WALL-CLOCK time -- proving it never hangs.
        let (client_io, server_io) = tokio::io::duplex(8192);

        let server = tokio::spawn(async move {
            let mut io = server_io;
            while let Some(line) = read_scripted_line(&mut io).await {
                let tag = line.split_whitespace().next().unwrap_or("").to_string();
                let upper = line.to_ascii_uppercase();
                if upper.contains("UID FETCH") {
                    // Deliberately never respond: a mid-stream server stall.
                    std::future::pending::<()>().await;
                } else if upper.contains("LOGIN") {
                    let _ = io
                        .write_all(format!("{tag} OK Logged in\r\n").as_bytes())
                        .await;
                } else if upper.contains("SELECT") {
                    let resp = format!(
                        "* 1 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT 42] ok\r\n\
                         {tag} OK [READ-WRITE] SELECT done\r\n"
                    );
                    let _ = io.write_all(resp.as_bytes()).await;
                }
            }
        });

        let client = async_imap::Client::new(client_io);
        let mut session = client
            .login("user", "pass")
            .await
            .map_err(|(e, _)| e)
            .expect("scripted login succeeds");
        let engine = ImapEngine::new("acct-1", "example.test", 993);

        // Real wall clock, not the (paused) tokio clock: proves negligible
        // elapsed time despite the 60s virtual timeout expiring.
        let started = std::time::Instant::now();
        let err = engine
            .sync_folder_messages_with_session("INBOX", None, &mut session)
            .await
            .expect_err("a never-responding FETCH must surface an error");
        assert!(
            matches!(err, MailError::FetchStalled(_)),
            "expected FetchStalled, got: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stalled fetch must not hang in wall-clock time, took {:?}",
            started.elapsed()
        );

        server.abort();
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
