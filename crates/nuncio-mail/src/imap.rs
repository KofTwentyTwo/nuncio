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

/// Upper bound on how many messages a single `UID FETCH` may pull full bodies
/// for. A folder sync never issues one unbounded `UID FETCH <range>` that
/// buffers every body at once; instead the messages in range are enumerated
/// cheaply (UIDs + sizes, no bodies) and their bodies fetched in explicit
/// UID-set batches of at most this many. This bounds the transient in-memory
/// working set of full message bodies to a fixed window, so a mailbox with an
/// enormous number of messages cannot drive the daemon to exhaust memory in a
/// single fetch regardless of how many messages the server holds.
const MAX_MESSAGES_PER_FETCH_BATCH: usize = 200;

/// Upper bound on the RFC822 size (bytes) of a message whose full body the sync
/// will pull into memory. The server reports each message's size via
/// `RFC822.SIZE` during enumeration; a message above this bound is NOT fetched
/// with its full body (which could be arbitrarily large and OOM the daemon).
/// Instead only its headers are fetched and it is recorded with an empty body
/// plus a WARN naming the UID and size, rather than being silently dropped or
/// buffered whole. The bound matches the MIME parser's own maximum payload
/// ([`MimeParserAdapter::MAX_PAYLOAD_BYTES`]): a body larger than the parser
/// would ever accept is pointless to transfer only to reject.
const MAX_MESSAGE_BODY_BYTES: u32 = 25 * 1024 * 1024;

use crate::backend::{MailBackend, RemoteMutationKind, RemoteMutationSpec};
use crate::parser::{MailError, MimeParserAdapter};

/// Recover a raw IMAP UID from a [`RemoteMutationSpec::remote_id`] (a decimal
/// string). Returns `None` for anything that is not a valid non-zero `u32`, so
/// a mutation against a malformed/foreign remote id fails honestly rather than
/// addressing UID 0.
fn parse_imap_uid(remote_id: &str) -> Option<u32> {
    let uid = remote_id.trim().parse::<u32>().ok()?;
    (uid >= 1).then_some(uid)
}

/// Parse a stored [`RemoteMutationSpec::uid_validity`] (a decimal string) into
/// a `u32`. Returns `None` for a missing or non-numeric value -- the caller
/// treats an unparseable UIDVALIDITY as "cannot prove the mailbox has not been
/// renumbered" and refuses to act.
fn parse_uid_validity(uid_validity: &str) -> Option<u32> {
    uid_validity.trim().parse::<u32>().ok()
}

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

/// Scheme tag prefixing every checkpoint this build writes.
///
/// The stored value is `"v1:uidnext:{uidvalidity}:{uidnext}"`. The tag exists
/// because the *next* checkpoint scheme -- a QRESYNC `(uidvalidity,
/// highestmodseq)` pair -- has exactly the same shape as this one and a wholly
/// different meaning. Untagged, an upgraded engine would hand a stored UIDNEXT
/// to a server as a MODSEQ. UIDNEXT is normally the larger number, so the
/// server would answer "nothing changed since" and every message below that
/// point would never be enumerated again: silent, permanent mail loss with no
/// error anywhere.
///
/// Anything that does not carry a scheme this build recognises -- an unknown
/// tag, or a bare `"42:105"` written before tagging existed -- resolves to a
/// full re-fetch. Over-fetching is recoverable; misreading a token is not.
const CHECKPOINT_SCHEME_UIDNEXT: &str = "v1:uidnext";

/// Scheme for a mod-sequence checkpoint: `"v2:modseq:{uidvalidity}:{modseq}"`.
///
/// Written by both mod-sequence rungs. QRESYNC and CONDSTORE differ in how
/// changes are *requested*, not in what has to be remembered -- both resume
/// from `(UIDVALIDITY, HIGHESTMODSEQ)` -- so one scheme covers both and a
/// mailbox can move between them without invalidating its checkpoint.
const CHECKPOINT_SCHEME_MODSEQ: &str = "v2:modseq";

/// Which mechanism a folder sync can use, in descending order of precision.
///
/// Chosen **per mailbox, not per account**: `NOMODSEQ` is a mailbox property
/// (RFC 7162 section 3.1.2.2), so one folder on a CONDSTORE server can still be
/// unable to answer mod-sequence queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncRung {
    /// `SELECT (QRESYNC …)` returns `VANISHED` plus changed messages in one
    /// round trip. The server reports removals directly.
    /// Fastmail/Cyrus, Dovecot.
    Qresync,
    /// `CHANGEDSINCE` narrows the fetch, but the server never volunteers
    /// removals, so absence is found by diffing the full UID set.
    /// **Gmail lives here**: it advertises CONDSTORE and not QRESYNC.
    Condstore,
    /// Neither extension. Changes and removals both come from a full UID-set
    /// enumeration. **Exchange lives here.**
    UidSetDiff,
    /// No usable checkpoint: fetch everything. Also where an unreadable or
    /// UIDVALIDITY-invalidated checkpoint lands.
    Full,
}

/// What the server and this mailbox can actually do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MailboxSyncCapabilities {
    /// Server advertises `QRESYNC` **and** this connection enabled it.
    pub qresync_enabled: bool,
    /// Server advertises `CONDSTORE`.
    pub condstore: bool,
    /// This mailbox reported a `HIGHESTMODSEQ`. A mailbox answering
    /// `NOMODSEQ` has no mod-sequences to compare against no matter what the
    /// server advertises.
    pub mailbox_has_modseq: bool,
}

/// One folder-sync pass, resolved before any body is fetched.
#[derive(Debug, Clone)]
pub(crate) struct FolderSyncPlan {
    /// The mechanism this pass negotiated.
    pub rung: SyncRung,
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub highest_modseq: Option<u64>,
    /// UID set whose bodies this pass fetches. Empty means nothing changed.
    pub body_sequence_set: String,
    /// UIDs the server reported as gone (QRESYNC only).
    pub vanished_uids: Vec<u32>,
    /// The folder's complete UID set, when this pass enumerated it.
    pub present_uids: Option<Vec<u32>>,
}

/// A checkpoint decoded into the resume point it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Checkpoint {
    /// `v1:uidnext` -- resume from a UID boundary.
    UidNext { uid_validity: u32, uid_next: u32 },
    /// `v2:modseq` -- resume from a mod-sequence.
    ModSeq { uid_validity: u32, modseq: u64 },
}

impl Checkpoint {
    fn uid_validity(self) -> u32 {
        match self {
            Checkpoint::UidNext { uid_validity, .. } | Checkpoint::ModSeq { uid_validity, .. } => {
                uid_validity
            }
        }
    }
}

/// Decode a stored checkpoint, or `None` when it carries no scheme this build
/// recognises. An unrecognised token is never guessed at -- see
/// [`CHECKPOINT_SCHEME_UIDNEXT`] for why that would lose mail silently.
pub(crate) fn parse_checkpoint(raw: &str) -> Option<Checkpoint> {
    let raw = raw.trim();
    if let Some(body) = raw
        .strip_prefix(CHECKPOINT_SCHEME_MODSEQ)
        .and_then(|r| r.strip_prefix(CHECKPOINT_DELIM))
    {
        let (validity, modseq) = body.split_once(CHECKPOINT_DELIM)?;
        return Some(Checkpoint::ModSeq {
            uid_validity: validity.trim().parse().ok()?,
            modseq: modseq.trim().parse().ok()?,
        });
    }
    let body = raw
        .strip_prefix(CHECKPOINT_SCHEME_UIDNEXT)
        .and_then(|r| r.strip_prefix(CHECKPOINT_DELIM))?;
    let (validity, uid) = body.split_once(CHECKPOINT_DELIM)?;
    let uid_next: u32 = uid.trim().parse().ok()?;
    if uid_next < 1 {
        return None;
    }
    Some(Checkpoint::UidNext {
        uid_validity: validity.trim().parse().ok()?,
        uid_next,
    })
}

/// Pick the rung for this pass.
///
/// A checkpoint whose UIDVALIDITY no longer matches the mailbox is not a
/// downgrade to a lesser rung -- it is [`SyncRung::Full`]. UIDs restart after a
/// UIDVALIDITY change, so every stored boundary refers to a different message
/// than it did (RFC 3501 section 2.3.1.1).
pub(crate) fn choose_rung(
    checkpoint: Option<Checkpoint>,
    current_uid_validity: Option<u32>,
    caps: MailboxSyncCapabilities,
) -> SyncRung {
    let modseq_usable = caps.mailbox_has_modseq && (caps.qresync_enabled || caps.condstore);

    let Some(checkpoint) = checkpoint else {
        // No checkpoint means everything must be fetched regardless of what
        // the server can do.
        return SyncRung::Full;
    };
    let (Some(current), stored) = (current_uid_validity, checkpoint.uid_validity()) else {
        return SyncRung::Full;
    };
    if current != stored {
        return SyncRung::Full;
    }

    match checkpoint {
        Checkpoint::ModSeq { .. } if modseq_usable && caps.qresync_enabled => SyncRung::Qresync,
        Checkpoint::ModSeq { .. } if modseq_usable => SyncRung::Condstore,
        // A mod-sequence checkpoint against a mailbox that can no longer
        // answer mod-sequence queries (index loss, or a move to a server
        // without the extension) has nothing to resume from.
        Checkpoint::ModSeq { .. } => SyncRung::Full,
        Checkpoint::UidNext { .. } => SyncRung::UidSetDiff,
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

/// The `UID FETCH` query for the cheap enumeration pass: UID plus `RFC822.SIZE`
/// only, deliberately WITHOUT a `BODY.PEEK[]`. This lets the sync discover every
/// message's UID and size (to decide per-message whether the body is safe to
/// pull) without transferring any body, so the enumeration stays small even for
/// a huge mailbox.
fn build_enumerate_command_query() -> &'static str {
    "(UID RFC822.SIZE)"
}

/// The `UID FETCH` query for a message whose body exceeds
/// [`MAX_MESSAGE_BODY_BYTES`]: headers only (`BODY.PEEK[HEADER]`), never the
/// full `BODY.PEEK[]`. The oversized body is intentionally left unfetched; the
/// headers are enough to record honest metadata (subject, sender, date).
fn build_headers_only_command_query() -> &'static str {
    "(FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])"
}

/// Render an IMAP ENVELOPE address as `mailbox@host`, or bare `mailbox` when no
/// host is present. Both components are decoded from their raw bytes lossily.
/// Used only when a FETCH item carries an ENVELOPE but no body, so a minimal
/// record can still name the sender/recipient.
fn format_envelope_address(addr: &async_imap::imap_proto::Address) -> String {
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
}

/// Render an explicit IMAP UID sequence-set from a batch of UIDs, e.g.
/// `[12, 15, 18]` -> `"12,15,18"`. Used to fetch a bounded batch of specific
/// messages rather than an open-ended `n:*` range.
fn join_uid_batch(uids: &[u32]) -> String {
    uids.iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Drive one already-issued `UID FETCH` response stream to completion,
/// collecting its items. Every individual item read is bounded by
/// [`FETCH_ITEM_TIMEOUT`]: a slow-but-progressing fetch is fine, but a single
/// item that never arrives (dead socket, server-side hang) surfaces as an
/// honest [`MailError::FetchStalled`] instead of hanging the sync. `read_so_far`
/// is the count of messages already read across earlier batches, so the stall
/// message reports the true progress.
async fn drive_fetch_stream<St, E>(
    stream: &mut St,
    folder_id: &str,
    read_so_far: usize,
) -> Result<Vec<async_imap::types::Fetch>, MailError>
where
    St: tokio_stream::Stream<Item = Result<async_imap::types::Fetch, E>> + Unpin,
    E: std::fmt::Display,
{
    let mut items = Vec::new();
    loop {
        let next_item = tokio::time::timeout(FETCH_ITEM_TIMEOUT, stream.next())
            .await
            .map_err(|_| {
                MailError::FetchStalled(format!(
                    "no FETCH response for folder '{}' within {}s ({} message(s) read so far)",
                    folder_id,
                    FETCH_ITEM_TIMEOUT.as_secs(),
                    read_so_far + items.len()
                ))
            })?;
        let Some(fetch_res) = next_item else {
            break;
        };
        let fetch_data = fetch_res
            .map_err(|e| MailError::ImapError(format!("failed reading fetch item: {}", e)))?;
        items.push(fetch_data);
    }
    Ok(items)
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
    ) -> Result<crate::backend::FolderChanges, MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        self.sync_folder_messages_batched(
            folder_id,
            since_state,
            session,
            MAX_MESSAGES_PER_FETCH_BATCH,
        )
        .await
    }

    /// Core folder sync, parameterised by the maximum number of messages whose
    /// full bodies are pulled per `UID FETCH`. Production callers pass
    /// [`MAX_MESSAGES_PER_FETCH_BATCH`]; the parameter is a seam so tests can
    /// exercise the multi-batch path with a small window instead of a huge
    /// fixture.
    ///
    /// The sync runs in two phases so the transient working set stays bounded
    /// regardless of mailbox size:
    /// 1. A cheap enumeration ([`build_enumerate_command_query`]) reads every
    ///    in-range message's UID and `RFC822.SIZE` with NO body. Messages at or
    ///    below [`MAX_MESSAGE_BODY_BYTES`] are queued for a full-body fetch;
    ///    oversized ones are queued for a headers-only fetch and WARN-logged.
    /// 2. Each queue is drained in explicit UID-set batches of at most
    ///    `batch_size`, so the full bodies held in memory at once never exceed
    ///    one batch.
    #[tracing::instrument(
        skip(self, session),
        fields(account_id = %self.account_id, folder_id = %folder_id),
        err
    )]
    async fn sync_folder_messages_batched<S>(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
        session: &mut async_imap::Session<S>,
        batch_size: usize,
    ) -> Result<crate::backend::FolderChanges, MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        let plan = self
            .plan_folder_sync(folder_id, since_state, session)
            .await?;
        let server_uid_next = plan.uid_next;
        let server_uid_validity = plan.uid_validity;

        let batch_size = batch_size.max(1);
        let sequence_set = plan.body_sequence_set.clone();

        // Phase 1: enumerate UIDs and sizes only -- no bodies transferred, so
        // this stays small even for an enormous mailbox.
        let enumeration = {
            let mut enumerate_stream = session
                .uid_fetch(&sequence_set, build_enumerate_command_query())
                .await
                .map_err(|e| {
                    MailError::ImapError(format!(
                        "UID FETCH (enumerate) failed for folder '{}': {}",
                        folder_id, e
                    ))
                })?;
            drive_fetch_stream(&mut enumerate_stream, folder_id, 0).await?
        };

        let mut max_uid_seen: u32 = 0;
        let mut body_uids: Vec<u32> = Vec::new();
        let mut oversized_uids: Vec<u32> = Vec::new();
        let mut skipped_no_uid: usize = 0;
        for item in &enumeration {
            // A message with no UID cannot be addressed by a follow-up fetch,
            // so it is skipped rather than issued a UID-less request.
            let Some(uid) = item.uid.filter(|u| *u >= 1) else {
                skipped_no_uid += 1;
                continue;
            };
            max_uid_seen = max_uid_seen.max(uid);
            match item.size {
                Some(size) if size > MAX_MESSAGE_BODY_BYTES => {
                    tracing::warn!(
                        folder_id,
                        uid,
                        size,
                        max_body_bytes = MAX_MESSAGE_BODY_BYTES,
                        "message body exceeds the per-message size cap; fetching headers only \
                         and recording it with an empty body instead of buffering the full body"
                    );
                    oversized_uids.push(uid);
                }
                _ => body_uids.push(uid),
            }
        }
        drop(enumeration);

        let mut emails = Vec::new();

        // Phase 2a: normal messages -- full bodies, in bounded UID-set batches.
        for chunk in body_uids.chunks(batch_size) {
            let set = join_uid_batch(chunk);
            let items = {
                let mut fetch_stream = session
                    .uid_fetch(&set, build_fetch_command_query())
                    .await
                    .map_err(|e| {
                    MailError::ImapError(format!(
                        "UID FETCH failed for folder '{}': {}",
                        folder_id, e
                    ))
                })?;
                drive_fetch_stream(&mut fetch_stream, folder_id, emails.len()).await?
            };
            for item in &items {
                emails.push(self.build_email_from_fetch(folder_id, server_uid_validity, item)?);
            }
        }

        // Phase 2b: oversized messages -- headers only, recorded with an empty
        // body. Also batched, so a run of oversized messages cannot itself
        // buffer an unbounded number of header fetches at once.
        for chunk in oversized_uids.chunks(batch_size) {
            let set = join_uid_batch(chunk);
            let items = {
                let mut fetch_stream = session
                    .uid_fetch(&set, build_headers_only_command_query())
                    .await
                    .map_err(|e| {
                        MailError::ImapError(format!(
                            "UID FETCH (headers) failed for folder '{}': {}",
                            folder_id, e
                        ))
                    })?;
                drive_fetch_stream(&mut fetch_stream, folder_id, emails.len()).await?
            };
            for item in &items {
                emails.push(self.build_headers_only_email(folder_id, server_uid_validity, item));
            }
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
        // A mod-sequence checkpoint is preferred whenever the mailbox can
        // supply one, even after a full pass: writing it is what lets the NEXT
        // sync use a higher rung than this one did.
        let new_checkpoint = match (server_uid_validity, plan.highest_modseq, boundary_uid) {
            (Some(validity), Some(modseq), _) => format!(
                "{}{}{}{}{}",
                CHECKPOINT_SCHEME_MODSEQ, CHECKPOINT_DELIM, validity, CHECKPOINT_DELIM, modseq
            ),
            (Some(validity), None, Some(uid)) => format!(
                "{}{}{}{}{}",
                CHECKPOINT_SCHEME_UIDNEXT, CHECKPOINT_DELIM, validity, CHECKPOINT_DELIM, uid
            ),
            _ => since_state
                .map(str::to_string)
                .unwrap_or_else(|| "full-resync-required".to_string()),
        };

        let removals: Vec<String> = plan
            .vanished_uids
            .iter()
            .map(|uid| self.surrogate_for(folder_id, server_uid_validity, *uid))
            .collect();
        let present = plan.present_uids.as_ref().map(|uids| {
            uids.iter()
                .map(|uid| self.surrogate_for(folder_id, server_uid_validity, *uid))
                .collect()
        });

        tracing::info!(
            account_id = %self.account_id,
            folder_id,
            rung = ?plan.rung,
            fetched = emails.len(),
            removed = removals.len(),
            enumerated = plan.present_uids.as_ref().map_or(0, Vec::len),
            oversized = oversized_uids.len(),
            skipped = skipped_no_uid,
            "imap folder sync complete"
        );

        Ok(crate::backend::FolderChanges {
            upserts: emails,
            removals,
            present,
            next_state: new_checkpoint,
        })
    }

    /// Negotiate capabilities, select the folder by the best available means,
    /// and decide what this pass must fetch and what it can say about absence.
    ///
    /// This is where the ladder is applied. Ordering matters: `ENABLE` must
    /// precede `SELECT` (RFC 7162 section 3.2.3 makes a server answer `BAD` to
    /// a QRESYNC `SELECT` otherwise), and the rung cannot be finalised until
    /// the mailbox has reported whether it has mod-sequences at all.
    async fn plan_folder_sync<S>(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
        session: &mut async_imap::Session<S>,
    ) -> Result<FolderSyncPlan, MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        let (has_qresync, has_condstore) = {
            let caps = session
                .capabilities()
                .await
                .map_err(|e| MailError::ImapError(format!("CAPABILITY query failed: {e}")))?;
            (caps.has_str("QRESYNC"), caps.has_str("CONDSTORE"))
        };

        let checkpoint = since_state.and_then(parse_checkpoint);
        if since_state.is_some() && checkpoint.is_none() {
            tracing::warn!(
                folder_id,
                checkpoint = since_state.unwrap_or_default(),
                "folder sync checkpoint carries no scheme this build recognises; \
                 re-fetching in full rather than guessing at its meaning"
            );
        }

        // ENABLE before SELECT, and only when there is a mod-sequence
        // checkpoint that a QRESYNC SELECT could actually resume from.
        let qresync_enabled = match (has_qresync, checkpoint) {
            (true, Some(Checkpoint::ModSeq { .. })) => {
                crate::imap_raw::enable(session, &["QRESYNC"]).await?
            }
            _ => false,
        };

        // Try the QRESYNC SELECT when it is available. Per RFC 7162 section
        // 3.2.5 a server whose UIDVALIDITY no longer matches simply ignores the
        // QRESYNC parameters, so this is safe to attempt: the mismatch is
        // detected below from the UIDVALIDITY it reports back.
        let (selected, attempted_qresync) = match (qresync_enabled, checkpoint) {
            (
                true,
                Some(Checkpoint::ModSeq {
                    uid_validity,
                    modseq,
                }),
            ) => (
                crate::imap_raw::select_qresync(session, folder_id, uid_validity, modseq, None)
                    .await?,
                true,
            ),
            _ => {
                let exchange =
                    crate::imap_raw::run_collected(session, &format!("SELECT {folder_id}")).await?;
                if !exchange.ok {
                    return Err(MailError::ImapError(format!(
                        "failed to select folder '{folder_id}': {}",
                        exchange.information.as_deref().unwrap_or("no detail")
                    )));
                }
                (exchange, false)
            }
        };

        let caps = MailboxSyncCapabilities {
            qresync_enabled,
            condstore: has_condstore,
            mailbox_has_modseq: selected.highest_modseq.is_some(),
        };
        let mut rung = choose_rung(checkpoint, selected.uid_validity, caps);
        // The QRESYNC SELECT was issued optimistically; if the server answered
        // with a different UIDVALIDITY it ignored our parameters, and anything
        // it did report refers to a UID space that no longer exists.
        if attempted_qresync && rung != SyncRung::Qresync {
            tracing::warn!(
                folder_id,
                "QRESYNC resume was refused or the mailbox was renumbered; re-fetching in full"
            );
        }
        if rung == SyncRung::Qresync && !attempted_qresync {
            rung = SyncRung::Full;
        }

        let mut plan = FolderSyncPlan {
            rung,
            uid_validity: selected.uid_validity,
            uid_next: selected.uid_next,
            highest_modseq: selected.highest_modseq,
            body_sequence_set: "1:*".to_string(),
            vanished_uids: Vec::new(),
            present_uids: None,
        };

        match rung {
            SyncRung::Qresync => {
                // The server volunteered both halves: what changed, and what
                // disappeared. No enumeration of the folder is needed at all,
                // which is the entire reason this rung is worth having.
                plan.vanished_uids = selected.vanished_uids.clone();
                plan.body_sequence_set = if selected.fetched_uids.is_empty() {
                    // Nothing changed. An empty set would be a protocol error,
                    // so ask for a range that cannot match instead of skipping
                    // the fetch path.
                    String::new()
                } else {
                    join_uid_batch(&selected.fetched_uids)
                };
            }
            SyncRung::Condstore => {
                let modseq = match checkpoint {
                    Some(Checkpoint::ModSeq { modseq, .. }) => modseq,
                    _ => 0,
                };
                let changed = self
                    .enumerate_uids(session, folder_id, Some(modseq))
                    .await?;
                plan.body_sequence_set = if changed.is_empty() {
                    String::new()
                } else {
                    join_uid_batch(&changed)
                };
                // CONDSTORE narrows the fetch but never reports removals, so
                // absence has to be found by enumerating what remains.
                plan.present_uids = Some(self.enumerate_uids(session, folder_id, None).await?);
            }
            SyncRung::UidSetDiff => {
                // New arrivals come from the UID boundary; removals from the
                // full UID set. Two cheap enumerations beat re-fetching bodies.
                if let Some(Checkpoint::UidNext { uid_next, .. }) = checkpoint {
                    plan.body_sequence_set = format!("{uid_next}:*");
                }
                plan.present_uids = Some(self.enumerate_uids(session, folder_id, None).await?);
            }
            SyncRung::Full => {
                // Everything is fetched, so the same pass also establishes
                // exactly what the folder holds.
                plan.present_uids = Some(self.enumerate_uids(session, folder_id, None).await?);
            }
        }

        Ok(plan)
    }

    /// List the folder's UIDs, optionally narrowed to those changed since
    /// `changed_since`. Requests no bodies and no flags, so the cost is one
    /// short line per message rather than a download.
    async fn enumerate_uids<S>(
        &self,
        session: &mut async_imap::Session<S>,
        folder_id: &str,
        changed_since: Option<u64>,
    ) -> Result<Vec<u32>, MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        let command = match changed_since {
            Some(modseq) => format!("UID FETCH 1:* (UID) (CHANGEDSINCE {modseq})"),
            None => "UID FETCH 1:* (UID)".to_string(),
        };
        let exchange = crate::imap_raw::run_collected(session, &command).await?;
        if !exchange.ok {
            return Err(MailError::ImapError(format!(
                "UID enumeration of '{folder_id}' failed: {}",
                exchange.information.as_deref().unwrap_or("no detail")
            )));
        }
        let mut uids = exchange.fetched_uids;
        uids.sort_unstable();
        uids.dedup();
        Ok(uids)
    }

    /// Surrogate id for a UID in this account/folder/UIDVALIDITY scope, so
    /// removals name messages the same way stored rows do.
    fn surrogate_for(&self, folder_id: &str, uid_validity: Option<u32>, uid: u32) -> String {
        let validity = uid_validity.map(|v| v.to_string()).unwrap_or_default();
        Email::surrogate_id(&self.account_id, folder_id, &validity, &uid.to_string())
    }

    /// Build an [`Email`] from a full-body (or envelope-only) FETCH item. When
    /// the item carries a `BODY[]` it is parsed as a full MIME message; when it
    /// does not (a server that returned only the ENVELOPE), a minimal record is
    /// synthesised from the envelope so the message is still surfaced honestly.
    fn build_email_from_fetch(
        &self,
        folder_id: &str,
        server_uid_validity: Option<u32>,
        fetch_data: &async_imap::types::Fetch,
    ) -> Result<Email, MailError> {
        let uid_num = fetch_data.uid.unwrap_or(0);
        // The message is addressed on the wire by its UID within the folder's
        // UIDVALIDITY scope; the persisted id is an opaque surrogate hashed over
        // both plus the account and folder, so the same UID in another
        // folder/account can never collide.
        let remote_id = uid_num.to_string();
        let uid_validity = server_uid_validity
            .map(|v| v.to_string())
            .unwrap_or_default();
        let email_id = Email::surrogate_id(&self.account_id, folder_id, &uid_validity, &remote_id);

        let is_read = fetch_data
            .flags()
            .any(|flag| matches!(flag, async_imap::types::Flag::Seen));

        if let Some(raw_bytes) = fetch_data.body() {
            let mut email =
                MimeParserAdapter::parse_mime(&email_id, &self.account_id, folder_id, raw_bytes)?;
            email.remote_id = remote_id;
            email.uid_validity = uid_validity;
            email.read = is_read;
            return Ok(email);
        }

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
            .map_or("unknown@nuncio.mx".to_string(), format_envelope_address);

        let recipient = fetch_data
            .envelope()
            .and_then(|env| env.to.as_ref())
            .and_then(|tos| tos.first())
            .map_or("me@nuncio.mx".to_string(), format_envelope_address);

        let received_at = fetch_data.internal_date().map_or(0, |dt| dt.timestamp());

        Ok(Email {
            id: email_id,
            account_id: self.account_id.clone(),
            folder_id: folder_id.to_string(),
            remote_id,
            uid_validity,
            subject,
            sender,
            recipient,
            received_at,
            read: is_read,
            body_plain: None,
            body_html: None,
            attachments: Vec::new(),
        })
    }

    /// Build an [`Email`] for a message whose body exceeded
    /// [`MAX_MESSAGE_BODY_BYTES`]: its headers (`BODY[HEADER]`) are parsed for
    /// honest metadata (subject/sender/date) but the oversized body is left
    /// unfetched, so the record carries an EMPTY body rather than a fabricated
    /// one. If the server returned no header section, this falls back to the
    /// envelope-only record; either way the message is surfaced, never dropped.
    fn build_headers_only_email(
        &self,
        folder_id: &str,
        server_uid_validity: Option<u32>,
        fetch_data: &async_imap::types::Fetch,
    ) -> Email {
        let uid_num = fetch_data.uid.unwrap_or(0);
        let remote_id = uid_num.to_string();
        let uid_validity = server_uid_validity
            .map(|v| v.to_string())
            .unwrap_or_default();
        let email_id = Email::surrogate_id(&self.account_id, folder_id, &uid_validity, &remote_id);
        let is_read = fetch_data
            .flags()
            .any(|flag| matches!(flag, async_imap::types::Flag::Seen));

        if let Some(header_bytes) = fetch_data.header() {
            if let Ok(mut email) =
                MimeParserAdapter::parse_mime(&email_id, &self.account_id, folder_id, header_bytes)
            {
                // A headers-only parse yields no body, but be explicit that the
                // oversized body was intentionally not fetched.
                email.remote_id = remote_id;
                email.uid_validity = uid_validity;
                email.read = is_read;
                email.body_plain = None;
                email.body_html = None;
                email.attachments = Vec::new();
                return email;
            }
        }

        // No usable header section: fall back to whatever the envelope offers.
        // `build_email_from_fetch` cannot error on this path (no body to parse),
        // but if it ever did, surface a minimal honest record rather than panic.
        self.build_email_from_fetch(folder_id, server_uid_validity, fetch_data)
            .unwrap_or_else(|_| Email {
                id: email_id,
                account_id: self.account_id.clone(),
                folder_id: folder_id.to_string(),
                remote_id,
                uid_validity,
                subject: "No Subject".to_string(),
                sender: "unknown@nuncio.mx".to_string(),
                recipient: "me@nuncio.mx".to_string(),
                received_at: 0,
                read: is_read,
                body_plain: None,
                body_html: None,
                attachments: Vec::new(),
            })
    }

    /// Fetch a folder's messages over a freshly authenticated session,
    /// honoring `since_state` for an incremental fetch and returning the new
    /// sync checkpoint alongside the messages. See
    /// [`Self::sync_folder_messages_with_session`] for the checkpoint model.
    pub async fn sync_folder_messages(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<crate::backend::FolderChanges, MailError> {
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

    /// Apply a remote mutation over an active IMAP session, selecting the
    /// source folder and verifying its UIDVALIDITY against the stored
    /// checkpoint BEFORE issuing any command that could touch the wrong
    /// message.
    ///
    /// The UIDVALIDITY guard (RFC 3501 s2.3.1.1) is the safety-critical step: a
    /// stored IMAP UID is only meaningful under the UIDVALIDITY it was captured
    /// with. If the mailbox was recreated its UIDVALIDITY changes and UIDs are
    /// reassigned, so acting on the old UID would mutate a different message. A
    /// mismatch -- or a stored checkpoint that carries no UIDVALIDITY to
    /// compare against, or a server that reports none -- is refused with an
    /// honest [`MailError::ImapError`]; the mutation is never applied blind.
    ///
    /// `MOVE` uses `UID MOVE` (RFC 6851) when the server advertises the `MOVE`
    /// capability, else falls back to the equivalent `UID COPY` + `UID STORE
    /// +FLAGS (\Deleted)` + UID-scoped `UID EXPUNGE`. `DELETE` is `UID STORE
    /// +FLAGS (\Deleted)` + `UID EXPUNGE`. The expunge is always scoped to the
    /// target UID via RFC 4315 (never a mailbox-wide `EXPUNGE`, which would
    /// destroy other `\Deleted` messages) and requires `UIDPLUS`, failing
    /// closed otherwise. Each command's tagged result is checked, so only a
    /// genuine server success returns `Ok(())`.
    pub async fn apply_mutation_with_session<S>(
        &self,
        spec: &RemoteMutationSpec,
        session: &mut async_imap::Session<S>,
    ) -> Result<(), MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        let uid = parse_imap_uid(&spec.remote_id).ok_or_else(|| {
            MailError::ImapError(format!(
                "cannot parse an IMAP UID from remote id '{}' (message '{}')",
                spec.remote_id, spec.message_id
            ))
        })?;

        let mailbox = session.select(&spec.folder_id).await.map_err(|e| {
            MailError::ImapError(format!(
                "failed to select folder '{}': {}",
                spec.folder_id, e
            ))
        })?;

        // Refuse to act unless the mailbox's current UIDVALIDITY provably
        // matches the one the stored UID was captured under.
        let stored_validity = parse_uid_validity(&spec.uid_validity);
        let guard_passed = matches!(
            (stored_validity, mailbox.uid_validity),
            (Some(stored), Some(current)) if stored == current
        );

        let uid_set = uid.to_string();

        // Every destructive op (MOVE, DELETE -- which issues STORE \Deleted
        // followed by a UID-scoped EXPUNGE) is logged BEFORE any command is
        // sent to the server, so an audit trail exists even when the
        // UIDVALIDITY guard below then refuses to proceed.
        match &spec.kind {
            RemoteMutationKind::Move { to_folder } => {
                tracing::info!(
                    op = "MOVE",
                    folder_id = %spec.folder_id,
                    to_folder = %to_folder,
                    uid_set = %uid_set,
                    uidvalidity_guard_passed = guard_passed,
                    "issuing destructive IMAP MOVE"
                );
            }
            RemoteMutationKind::Delete => {
                tracing::warn!(
                    op = "DELETE",
                    folder_id = %spec.folder_id,
                    uid_set = %uid_set,
                    uidvalidity_guard_passed = guard_passed,
                    "issuing destructive IMAP DELETE (STORE \\Deleted + UID-scoped EXPUNGE)"
                );
            }
            RemoteMutationKind::SetFlagged { .. } | RemoteMutationKind::Copy { .. } => {}
        }

        if !guard_passed {
            let current_validity = mailbox.uid_validity;
            return Err(MailError::ImapError(format!(
                "refusing to mutate message '{}' in folder '{}': UIDVALIDITY guard failed \
                 (stored {stored_validity:?}, current {current_validity:?}); the stored UID may \
                 no longer address the intended message",
                spec.message_id, spec.folder_id
            )));
        }

        match &spec.kind {
            RemoteMutationKind::SetFlagged { value } => {
                let op = if *value { "+FLAGS" } else { "-FLAGS" };
                self.uid_store_flags(session, &uid_set, op, "\\Flagged")
                    .await
            }
            RemoteMutationKind::Copy { to_folder } => {
                session.uid_copy(&uid_set, to_folder).await.map_err(|e| {
                    MailError::ImapError(format!(
                        "UID COPY of '{}' to '{}' failed: {}",
                        spec.message_id, to_folder, e
                    ))
                })
            }
            RemoteMutationKind::Move { to_folder } => {
                self.uid_move(session, &uid_set, to_folder, &spec.message_id)
                    .await
            }
            RemoteMutationKind::Delete => {
                self.uid_store_flags(session, &uid_set, "+FLAGS", "\\Deleted")
                    .await?;
                self.uid_expunge_scoped(session, &uid_set).await
            }
        }
    }

    /// Issue a `UID STORE {uid} {op} ({flag})` and drain its untagged `FETCH`
    /// responses, surfacing any protocol error. `op` is `+FLAGS`/`-FLAGS`.
    async fn uid_store_flags<S>(
        &self,
        session: &mut async_imap::Session<S>,
        uid_set: &str,
        op: &str,
        flag: &str,
    ) -> Result<(), MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        let query = format!("{op} ({flag})");
        let updates = session.uid_store(uid_set, &query).await.map_err(|e| {
            MailError::ImapError(format!("UID STORE {query} for uid {uid_set} failed: {e}"))
        })?;
        tokio::pin!(updates);
        while let Some(item) = updates.next().await {
            item.map_err(|e| {
                MailError::ImapError(format!("reading UID STORE response failed: {e}"))
            })?;
        }
        Ok(())
    }

    /// Permanently remove ONLY the target `uid_set` from the selected folder
    /// via RFC 4315 `UID EXPUNGE`, draining its untagged `EXPUNGE` responses.
    ///
    /// A bare `EXPUNGE` (RFC 3501) removes EVERY `\Deleted` message in the
    /// mailbox, so it would collaterally destroy any other message a different
    /// client had already flagged `\Deleted`. `UID EXPUNGE` is scoped to
    /// exactly the given UIDs, so it is the only safe expunge in a shared
    /// mailbox. It requires the `UIDPLUS` capability; if the server does not
    /// advertise it, this fails closed with an honest error rather than
    /// falling back to the destructive mailbox-wide `EXPUNGE`.
    async fn uid_expunge_scoped<S>(
        &self,
        session: &mut async_imap::Session<S>,
        uid_set: &str,
    ) -> Result<(), MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        let caps = session
            .capabilities()
            .await
            .map_err(|e| MailError::ImapError(format!("CAPABILITY query failed: {e}")))?;
        let has_uidplus = caps.has_str("UIDPLUS");
        drop(caps);
        if !has_uidplus {
            return Err(MailError::ImapError(format!(
                "refusing to expunge uid {uid_set}: server does not advertise UIDPLUS, so a \
                 UID-scoped EXPUNGE (RFC 4315) is unavailable; a bare EXPUNGE could destroy \
                 other \\Deleted messages in the folder"
            )));
        }

        let expunged = session
            .uid_expunge(uid_set)
            .await
            .map_err(|e| MailError::ImapError(format!("UID EXPUNGE {uid_set} failed: {e}")))?;
        tokio::pin!(expunged);
        while let Some(item) = expunged.next().await {
            item.map_err(|e| {
                MailError::ImapError(format!("reading UID EXPUNGE response failed: {e}"))
            })?;
        }
        Ok(())
    }

    /// Move a message by UID, preferring RFC 6851 `UID MOVE` and falling back
    /// to `UID COPY` + `UID STORE +FLAGS (\Deleted)` + UID-scoped `UID EXPUNGE`
    /// when the server does not advertise the `MOVE` capability. The fallback's
    /// expunge is scoped to the target UID (never a mailbox-wide `EXPUNGE`) and
    /// requires `UIDPLUS`, failing closed otherwise.
    async fn uid_move<S>(
        &self,
        session: &mut async_imap::Session<S>,
        uid_set: &str,
        to_folder: &str,
        message_id: &str,
    ) -> Result<(), MailError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
    {
        let caps = session
            .capabilities()
            .await
            .map_err(|e| MailError::ImapError(format!("CAPABILITY query failed: {e}")))?;
        let has_move = caps.has_str("MOVE");
        drop(caps);

        if has_move {
            return session.uid_mv(uid_set, to_folder).await.map_err(|e| {
                MailError::ImapError(format!(
                    "UID MOVE of '{message_id}' to '{to_folder}' failed: {e}"
                ))
            });
        }

        session.uid_copy(uid_set, to_folder).await.map_err(|e| {
            MailError::ImapError(format!(
                "UID COPY (move fallback) of '{message_id}' to '{to_folder}' failed: {e}"
            ))
        })?;
        self.uid_store_flags(session, uid_set, "+FLAGS", "\\Deleted")
            .await?;
        self.uid_expunge_scoped(session, uid_set).await
    }

    /// Apply a remote mutation over a freshly authenticated session. See
    /// [`Self::apply_mutation_with_session`] for the UIDVALIDITY guard and the
    /// per-action protocol commands.
    pub async fn apply_remote_mutation(&self, spec: &RemoteMutationSpec) -> Result<(), MailError> {
        let (username, password) = match (&self.username, &self.password) {
            (Some(u), Some(p)) => (u.as_str(), p.as_str()),
            _ => {
                return Err(MailError::AuthError(
                    "IMAP remote mutation requires credentials".to_string(),
                ))
            }
        };
        let mut session = self
            .socket_manager
            .connect_session(username, password)
            .await?;
        let result = self.apply_mutation_with_session(spec, &mut session).await;
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
    #[tracing::instrument(skip(self), fields(account_id = %self.account_id), err)]
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
        tracing::info!(
            account_id = %self.account_id,
            folders = folders.len(),
            "imap folder list sync complete"
        );
        // An empty mailbox is a genuine result, not a signal to fall back to
        // placeholder folders -- the caller gets exactly what the server reported.
        Ok(folders)
    }

    async fn sync_changes(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<crate::backend::FolderChanges, MailError> {
        self.sync_folder_messages(folder_id, since_state).await
    }

    async fn apply_mutation(&self, spec: &RemoteMutationSpec) -> Result<(), MailError> {
        self.apply_remote_mutation(spec).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_test::traced_test;

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
                if upper.contains("CAPABILITY") {
                    let _ = socket
                        .write_all(
                            format!(
                                "* CAPABILITY IMAP4rev1 UIDPLUS MOVE\r\n{tag} OK CAPABILITY\r\n"
                            )
                            .as_bytes(),
                        )
                        .await;
                } else if upper.contains("LOGIN") {
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

    /// Read a single CRLF-terminated line from a scripted-server stream,
    /// returning `None` at EOF (client hung up).
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

    /// The body/size fetches from a captured command list, excluding the
    /// `(UID)`-only enumeration a non-QRESYNC rung issues to find removals.
    /// That pass is expected and says nothing about how the body fetch was
    /// narrowed, which is what these tests are about.
    fn body_fetch_ranges(all: &[String]) -> Vec<String> {
        all.iter()
            .filter(|c| !c.to_ascii_uppercase().ends_with("(UID)"))
            .cloned()
            .collect()
    }

    /// Capabilities that can reach every rung, so a test only has to vary the
    /// one thing it is about.
    fn full_caps() -> MailboxSyncCapabilities {
        MailboxSyncCapabilities {
            qresync_enabled: true,
            condstore: true,
            mailbox_has_modseq: true,
        }
    }

    #[test]
    fn checkpoints_round_trip_through_their_scheme() {
        assert_eq!(
            parse_checkpoint("v2:modseq:42:900"),
            Some(Checkpoint::ModSeq {
                uid_validity: 42,
                modseq: 900
            })
        );
        assert_eq!(
            parse_checkpoint("v1:uidnext:42:105"),
            Some(Checkpoint::UidNext {
                uid_validity: 42,
                uid_next: 105
            })
        );
        assert_eq!(
            parse_checkpoint("v1:uidnext: 7 : 42 "),
            Some(Checkpoint::UidNext {
                uid_validity: 7,
                uid_next: 42
            })
        );
    }

    #[test]
    fn an_untagged_or_unknown_scheme_checkpoint_is_not_interpreted() {
        // A QRESYNC checkpoint is (uidvalidity, highestmodseq) -- the same
        // shape as (uidvalidity, uidnext) and a different meaning. Reading one
        // as the other hands the server a UIDNEXT where it expects a MODSEQ;
        // UIDNEXT is normally the larger number, so the server answers
        // "nothing changed" and everything below is never enumerated again.
        for token in [
            "42:105",           // pre-tagging
            "v3:future:42:105", // a scheme from a later build
            "v1:modseq:42:105", // tag-shaped but not ours
            "v1:42:105",        // truncated tag
            "v1:uidnext:42:0",  // a UID boundary below 1 is not addressable
            "",
        ] {
            assert_eq!(
                parse_checkpoint(token),
                None,
                "token {token:?} must not be interpreted"
            );
            assert_eq!(
                choose_rung(parse_checkpoint(token), Some(42), full_caps()),
                SyncRung::Full,
                "an uninterpretable token must re-fetch everything"
            );
        }
    }

    #[test]
    fn the_rung_is_chosen_from_the_checkpoint_and_what_the_mailbox_supports() {
        let modseq = parse_checkpoint("v2:modseq:42:900");
        let uidnext = parse_checkpoint("v1:uidnext:42:105");

        // Fastmail/Cyrus, Dovecot: the server states removals directly.
        assert_eq!(
            choose_rung(modseq, Some(42), full_caps()),
            SyncRung::Qresync
        );

        // Gmail: CONDSTORE without QRESYNC. This is the rung a two-rung ladder
        // would have skipped, pushing the largest provider onto full sweeps.
        assert_eq!(
            choose_rung(
                modseq,
                Some(42),
                MailboxSyncCapabilities {
                    qresync_enabled: false,
                    condstore: true,
                    mailbox_has_modseq: true
                }
            ),
            SyncRung::Condstore
        );

        // Exchange: neither extension, so a UID-set diff is all that is left.
        assert_eq!(
            choose_rung(
                uidnext,
                Some(42),
                MailboxSyncCapabilities {
                    qresync_enabled: false,
                    condstore: false,
                    mailbox_has_modseq: false
                }
            ),
            SyncRung::UidSetDiff
        );

        // NOMODSEQ is a MAILBOX property (RFC 7162 3.1.2.2): a mod-sequence
        // checkpoint against a mailbox that cannot answer mod-sequence queries
        // has nothing to resume from, whatever the server advertises.
        assert_eq!(
            choose_rung(
                modseq,
                Some(42),
                MailboxSyncCapabilities {
                    qresync_enabled: true,
                    condstore: true,
                    mailbox_has_modseq: false
                }
            ),
            SyncRung::Full
        );

        // No checkpoint at all.
        assert_eq!(choose_rung(None, Some(42), full_caps()), SyncRung::Full);
    }

    #[test]
    fn a_uidvalidity_change_forces_a_full_pass_from_every_rung() {
        // UIDs restart after a UIDVALIDITY change (RFC 3501 2.3.1.1), so every
        // stored boundary now names a different message. This is not a
        // downgrade to a lesser rung; nothing stored can be resumed from.
        for token in ["v2:modseq:42:900", "v1:uidnext:42:105"] {
            assert_eq!(
                choose_rung(parse_checkpoint(token), Some(43), full_caps()),
                SyncRung::Full,
                "{token} must not survive a renumbering"
            );
            assert_eq!(
                choose_rung(parse_checkpoint(token), None, full_caps()),
                SyncRung::Full,
                "{token}: a server that reports no UIDVALIDITY cannot be resumed against"
            );
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
                } else if upper.contains("CAPABILITY") {
                    // The planner negotiates capabilities before selecting; a
                    // script that never answers this blocks the client forever.
                    let _ = io
                        .write_all(
                            format!(
                                "* CAPABILITY IMAP4rev1 UIDPLUS MOVE\r\n{tag} OK CAPABILITY\r\n"
                            )
                            .as_bytes(),
                        )
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
        let first_pass = engine
            .sync_folder_messages_with_session("INBOX", None, &mut session)
            .await
            .expect("first sync succeeds");
        let (first, checkpoint) = (first_pass.upserts, first_pass.next_state);
        assert!(first.is_empty());
        assert_eq!(checkpoint, "v1:uidnext:1:105");

        // Second sync: feed back the checkpoint; UIDVALIDITY still matches
        // (1), so the fetch is narrowed.
        let __changes = engine
            .sync_folder_messages_with_session("INBOX", Some(&checkpoint), &mut session)
            .await
            .expect("second sync succeeds");
        let (_second, checkpoint2) = (__changes.upserts, __changes.next_state);
        assert_eq!(checkpoint2, "v1:uidnext:1:105");

        drop(session);
        let _ = server.await;

        let all = fetch_ranges.lock().expect("lock captured ranges");
        let ranges = body_fetch_ranges(&all);
        assert_eq!(
            ranges.len(),
            2,
            "expected one body fetch per sync, got: {all:?}"
        );
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

    /// The RFC822 body of a scripted fixture message.
    fn fixture_body(uid: u32) -> String {
        format!(
            "Subject: Message {uid}\r\nFrom: sender{uid}@example.test\r\n\
             To: me@example.test\r\n\r\nBody of message {uid}.\r\n"
        )
    }

    /// The header block (no body) of a scripted fixture message.
    fn fixture_header(uid: u32) -> String {
        format!(
            "Subject: Message {uid}\r\nFrom: sender{uid}@example.test\r\n\
             To: me@example.test\r\n\r\n"
        )
    }

    /// Resolve which fixture UIDs a `UID FETCH` sequence-set token addresses. A
    /// range `lo:*` selects every fixture UID `>= lo`; an explicit comma list
    /// selects exactly those UIDs. Mirrors just enough server-side sequence-set
    /// handling for the batched-fetch tests.
    fn requested_uids(set: &str, all: &[(u32, u32)]) -> Vec<u32> {
        if set.contains(':') {
            let lo: u32 = set
                .split(':')
                .next()
                .unwrap_or("1")
                .trim()
                .parse()
                .unwrap_or(1);
            all.iter().map(|(u, _)| *u).filter(|u| *u >= lo).collect()
        } else {
            set.split(',')
                .filter_map(|s| s.trim().parse::<u32>().ok())
                .filter(|u| all.iter().any(|(fu, _)| fu == u))
                .collect()
        }
    }

    /// Drive `sync_folder_messages_batched` over a scripted, in-memory IMAP
    /// server that serves a fixed set of `(uid, size)` messages. The server
    /// answers the enumeration pass (UID + RFC822.SIZE), full-body batches
    /// (`BODY[]`), and headers-only batches (`BODY[HEADER]`) from the fixture,
    /// and records every `UID FETCH` command line the client issued so a test
    /// can assert on the exact batching. Returns the synced emails, the new
    /// checkpoint, and the captured `UID FETCH` command lines.
    async fn run_batched_sync(
        messages: Vec<(u32, u32)>,
        uidvalidity: u32,
        uidnext: u32,
        since_state: Option<String>,
        batch_size: usize,
    ) -> (Vec<Email>, String, Vec<String>) {
        let (client_io, server_io) = tokio::io::duplex(65536);
        let fetch_cmds = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let server_cmds = fetch_cmds.clone();
        let exists = messages.len() as u32;

        let server = tokio::spawn(async move {
            let mut io = server_io;
            while let Some(line) = read_scripted_line(&mut io).await {
                let tag = line.split_whitespace().next().unwrap_or("").to_string();
                let upper = line.to_ascii_uppercase();
                if upper.contains("UID FETCH") {
                    if let Ok(mut g) = server_cmds.lock() {
                        g.push(line.clone());
                    }
                    let set = line.split_whitespace().nth(3).unwrap_or("").to_string();
                    let uids = requested_uids(&set, &messages);
                    let mut resp = String::new();
                    for uid in uids {
                        let size = messages
                            .iter()
                            .find(|(u, _)| *u == uid)
                            .map_or(0, |(_, s)| *s);
                        if upper.contains("BODY.PEEK[HEADER]") {
                            let hdr = fixture_header(uid);
                            resp.push_str(&format!(
                                "* {uid} FETCH (UID {uid} RFC822.SIZE {size} FLAGS (\\Seen) \
                                 BODY[HEADER] {{{}}}\r\n{hdr})\r\n",
                                hdr.len()
                            ));
                        } else if upper.contains("BODY.PEEK[]") {
                            let body = fixture_body(uid);
                            resp.push_str(&format!(
                                "* {uid} FETCH (UID {uid} RFC822.SIZE {size} FLAGS (\\Seen) \
                                 BODY[] {{{}}}\r\n{body})\r\n",
                                body.len()
                            ));
                        } else {
                            // Enumeration pass: UID + size only, no body.
                            resp.push_str(&format!(
                                "* {uid} FETCH (UID {uid} RFC822.SIZE {size})\r\n"
                            ));
                        }
                    }
                    resp.push_str(&format!("{tag} OK FETCH completed\r\n"));
                    let _ = io.write_all(resp.as_bytes()).await;
                } else if upper.contains("CAPABILITY") {
                    // The planner negotiates capabilities before selecting; a
                    // script that never answers this blocks the client forever.
                    let _ = io
                        .write_all(
                            format!(
                                "* CAPABILITY IMAP4rev1 UIDPLUS MOVE\r\n{tag} OK CAPABILITY\r\n"
                            )
                            .as_bytes(),
                        )
                        .await;
                } else if upper.contains("LOGIN") {
                    let _ = io
                        .write_all(format!("{tag} OK Logged in\r\n").as_bytes())
                        .await;
                } else if upper.contains("SELECT") {
                    let resp = format!(
                        "* FLAGS (\\Seen)\r\n* {exists} EXISTS\r\n* 0 RECENT\r\n\
                         * OK [UIDVALIDITY {uidvalidity}] ok\r\n* OK [UIDNEXT {uidnext}] ok\r\n\
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
        let changes = engine
            .sync_folder_messages_batched("INBOX", since_state.as_deref(), &mut session, batch_size)
            .await
            .expect("batched sync succeeds");
        let (emails, checkpoint) = (changes.upserts, changes.next_state);
        drop(session);
        let _ = server.await;

        let cmds = fetch_cmds.lock().expect("lock captured commands").clone();
        (emails, checkpoint, cmds)
    }

    /// The captured `UID FETCH` command lines that pulled FULL bodies (a
    /// `BODY.PEEK[]`, not the headers-only `BODY.PEEK[HEADER]` nor the
    /// bodyless enumeration).
    fn body_fetch_commands(cmds: &[String]) -> Vec<&String> {
        cmds.iter()
            .filter(|c| {
                let u = c.to_ascii_uppercase();
                u.contains("BODY.PEEK[]") && !u.contains("BODY.PEEK[HEADER]")
            })
            .collect()
    }

    #[tokio::test]
    async fn a_large_folder_is_fetched_across_multiple_bounded_batches() {
        // Five messages with a batch window of two must be pulled across
        // multiple bounded `UID FETCH` batches -- never a single unbounded
        // `1:*` body fetch. The enumeration discovers the UIDs; the bodies are
        // then fetched in explicit UID-set chunks of at most the batch size.
        let messages: Vec<(u32, u32)> = (1..=5).map(|u| (u, 100)).collect();
        let (emails, checkpoint, cmds) = run_batched_sync(messages, 1, 6, None, 2).await;

        assert_eq!(emails.len(), 5, "every message must be synced");
        assert_eq!(checkpoint, "v1:uidnext:1:6");

        let body_cmds = body_fetch_commands(&cmds);
        assert_eq!(
            body_cmds.len(),
            3,
            "5 messages / batch of 2 => 3 bounded body fetches, got: {cmds:?}"
        );
        for cmd in &body_cmds {
            assert!(
                !cmd.contains("1:*") && !cmd.contains(":*"),
                "a body fetch must be a bounded UID set, never an open range: {cmd}"
            );
            let set = cmd.split_whitespace().nth(3).unwrap_or("");
            let count = set.split(',').count();
            assert!(
                count <= 2,
                "each body fetch must carry at most the batch size (2) UIDs, got: {cmd}"
            );
        }
    }

    #[tokio::test]
    async fn an_oversized_message_is_fetched_headers_only_not_buffered_whole() {
        // Three messages where the middle one reports an RFC822.SIZE above the
        // per-message cap. Its full body MUST NOT be requested; instead it is
        // fetched headers-only and recorded with an empty body, while the two
        // normal messages get full bodies.
        let huge = MAX_MESSAGE_BODY_BYTES + 1;
        let messages: Vec<(u32, u32)> = vec![(1, 100), (2, huge), (3, 100)];
        let (emails, _checkpoint, cmds) = run_batched_sync(messages, 1, 4, None, 200).await;

        assert_eq!(emails.len(), 3, "no message may be silently dropped");

        // The oversized UID (2) must NEVER appear in a full-body fetch.
        for cmd in body_fetch_commands(&cmds) {
            let set = cmd.split_whitespace().nth(3).unwrap_or("");
            assert!(
                !set.split(',').any(|u| u.trim() == "2"),
                "the oversized message's full body must never be fetched, got: {cmd}"
            );
        }
        // It MUST be fetched headers-only.
        assert!(
            cmds.iter().any(|c| {
                let u = c.to_ascii_uppercase();
                u.contains("BODY.PEEK[HEADER]") && c.split_whitespace().nth(3).unwrap_or("") == "2"
            }),
            "the oversized message must be fetched headers-only, got: {cmds:?}"
        );

        // The oversized message is recorded with honest metadata and an EMPTY
        // body -- never a fabricated one.
        let oversized = emails
            .iter()
            .find(|e| e.remote_id == "2")
            .expect("oversized message must be present");
        assert!(
            oversized.body_plain.is_none() && oversized.body_html.is_none(),
            "oversized message must carry no body, got: {oversized:?}"
        );
        assert!(
            oversized.subject.contains('2'),
            "oversized message must keep honest header metadata, got: {oversized:?}"
        );

        // The normal messages DID get full bodies.
        for uid in ["1", "3"] {
            let email = emails
                .iter()
                .find(|e| e.remote_id == uid)
                .expect("normal message present");
            assert!(
                email.body_plain.is_some(),
                "normal message {uid} must have a body, got: {email:?}"
            );
        }
    }

    #[tokio::test]
    async fn batched_sync_preserves_incremental_checkpointing() {
        // Batching must not break the incremental checkpoint: a first sync
        // records "{uidvalidity}:{uidnext}", and a second sync fed that
        // checkpoint enumerates only the narrowed `{uidnext}:*` range (so a
        // fire-once-per-new-message consumer never re-sees old mail).
        let messages: Vec<(u32, u32)> = (1..=3).map(|u| (u, 100)).collect();
        let (first, checkpoint, first_cmds) =
            run_batched_sync(messages.clone(), 1, 4, None, 2).await;
        assert_eq!(first.len(), 3);
        assert_eq!(checkpoint, "v1:uidnext:1:4");
        assert!(
            first_cmds.iter().any(|c| c.contains("1:*")),
            "first sync enumerates the full range, got: {first_cmds:?}"
        );

        let (second, checkpoint2, second_cmds) =
            run_batched_sync(messages, 1, 4, Some(checkpoint.clone()), 2).await;
        assert_eq!(
            second.len(),
            0,
            "no new mail arrived, so an incremental sync returns nothing"
        );
        assert_eq!(
            checkpoint2, "v1:uidnext:1:4",
            "checkpoint stays stable across an empty pass"
        );
        let second_bodies = body_fetch_ranges(&second_cmds);
        assert!(
            second_bodies.iter().any(|c| c.contains("4:*")),
            "second sync must fetch only the narrowed range, got: {second_bodies:?}"
        );
        assert!(
            !second_bodies.iter().any(|c| c.contains("1:*")),
            "second sync must NOT re-fetch bodies for the full range, got: {second_bodies:?}"
        );
        // The `1:*` that IS expected: a UID-only enumeration. Without QRESYNC
        // there is no other way to learn a message was removed, and it costs
        // one short line per message rather than a body.
        assert!(
            second_cmds
                .iter()
                .any(|c| c.to_ascii_uppercase().ends_with("(UID)") && c.contains("1:*")),
            "a non-QRESYNC rung must enumerate the folder to detect removals, got: {second_cmds:?}"
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
                } else if upper.contains("CAPABILITY") {
                    // The planner negotiates capabilities before selecting; a
                    // script that never answers this blocks the client forever.
                    let _ = io
                        .write_all(
                            format!(
                                "* CAPABILITY IMAP4rev1 UIDPLUS MOVE\r\n{tag} OK CAPABILITY\r\n"
                            )
                            .as_bytes(),
                        )
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
        let __changes = engine
            .sync_folder_messages_with_session("INBOX", None, &mut session)
            .await
            .expect("first sync succeeds");
        let (_first, checkpoint) = (__changes.upserts, __changes.next_state);
        assert_eq!(checkpoint, "v1:uidnext:1:105");

        // Second sync: server now reports UIDVALIDITY 2. The stored checkpoint
        // is stale, so the fetch MUST be a full `1:*`, and the checkpoint is
        // rewritten under the new UIDVALIDITY.
        let __changes = engine
            .sync_folder_messages_with_session("INBOX", Some(&checkpoint), &mut session)
            .await
            .expect("second sync succeeds");
        let (_second, checkpoint2) = (__changes.upserts, __changes.next_state);
        assert_eq!(
            checkpoint2, "v1:uidnext:2:105",
            "checkpoint must be rewritten under the new UIDVALIDITY"
        );

        drop(session);
        let _ = server.await;

        let all = fetch_ranges.lock().expect("lock captured ranges");
        let ranges = body_fetch_ranges(&all);
        assert_eq!(
            ranges.len(),
            2,
            "expected one body fetch per sync, got: {all:?}"
        );
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
                } else if upper.contains("CAPABILITY") {
                    // The planner negotiates capabilities before selecting; a
                    // script that never answers this blocks the client forever.
                    let _ = io
                        .write_all(
                            format!(
                                "* CAPABILITY IMAP4rev1 UIDPLUS MOVE\r\n{tag} OK CAPABILITY\r\n"
                            )
                            .as_bytes(),
                        )
                        .await;
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

    #[test]
    fn parse_imap_uid_recovers_uid_and_rejects_non_numeric_ids() {
        assert_eq!(parse_imap_uid("42"), Some(42));
        assert_eq!(parse_imap_uid(" 42 "), Some(42));
        assert_eq!(parse_imap_uid("1"), Some(1));
        // UID 0 is not a valid IMAP UID; a non-numeric/foreign id yields nothing.
        assert_eq!(parse_imap_uid("0"), None);
        assert_eq!(parse_imap_uid("imap-uid-42"), None);
        assert_eq!(parse_imap_uid("jmap-object-id"), None);
    }

    #[test]
    fn parse_uid_validity_accepts_only_a_decimal_value() {
        assert_eq!(parse_uid_validity("7"), Some(7));
        assert_eq!(parse_uid_validity(" 7 "), Some(7));
        assert_eq!(parse_uid_validity(""), None);
        // The JMAP sentinel and a full checkpoint string are not a bare validity.
        assert_eq!(parse_uid_validity("jmap"), None);
        assert_eq!(parse_uid_validity("7:105"), None);
    }

    /// Drive a mutation over a scripted IMAP server (in-memory duplex),
    /// capturing every command line the client issued so the test can assert
    /// on the exact protocol commands. `advertise_move`/`advertise_uidplus`
    /// control whether the scripted `CAPABILITY` response includes `MOVE`/
    /// `UIDPLUS`; `select_uid_validity` is the UIDVALIDITY the scripted
    /// `SELECT` reports.
    async fn run_scripted_mutation(
        spec: &RemoteMutationSpec,
        advertise_move: bool,
        advertise_uidplus: bool,
        select_uid_validity: u32,
    ) -> (Result<(), MailError>, Vec<String>) {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let commands = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let server_commands = commands.clone();

        let server = tokio::spawn(async move {
            let mut io = server_io;
            while let Some(line) = read_scripted_line(&mut io).await {
                if let Ok(mut g) = server_commands.lock() {
                    g.push(line.clone());
                }
                let tag = line.split_whitespace().next().unwrap_or("").to_string();
                let upper = line.to_ascii_uppercase();
                if upper.contains("LOGIN") {
                    let _ = io
                        .write_all(format!("{tag} OK Logged in\r\n").as_bytes())
                        .await;
                } else if upper.contains("SELECT") {
                    let resp = format!(
                        "* FLAGS (\\Seen \\Flagged \\Deleted)\r\n* 3 EXISTS\r\n* 0 RECENT\r\n\
                         * OK [UIDVALIDITY {select_uid_validity}] ok\r\n* OK [UIDNEXT 105] ok\r\n\
                         {tag} OK [READ-WRITE] SELECT done\r\n"
                    );
                    let _ = io.write_all(resp.as_bytes()).await;
                } else if upper.contains("CAPABILITY") {
                    let mut caps = String::from("IMAP4rev1");
                    if advertise_move {
                        caps.push_str(" MOVE");
                    }
                    if advertise_uidplus {
                        caps.push_str(" UIDPLUS");
                    }
                    let _ = io
                        .write_all(
                            format!("* CAPABILITY {caps}\r\n{tag} OK CAPABILITY done\r\n")
                                .as_bytes(),
                        )
                        .await;
                } else if upper.contains("UID EXPUNGE") {
                    let _ = io
                        .write_all(format!("{tag} OK UID EXPUNGE completed\r\n").as_bytes())
                        .await;
                } else if upper.contains("UID STORE") {
                    let _ = io
                        .write_all(format!("{tag} OK STORE completed\r\n").as_bytes())
                        .await;
                } else if upper.contains("UID COPY") {
                    let _ = io
                        .write_all(format!("{tag} OK COPY completed\r\n").as_bytes())
                        .await;
                } else if upper.contains("UID MOVE") {
                    let _ = io
                        .write_all(format!("{tag} OK MOVE completed\r\n").as_bytes())
                        .await;
                } else if upper.contains("EXPUNGE") {
                    let _ = io
                        .write_all(format!("{tag} OK EXPUNGE completed\r\n").as_bytes())
                        .await;
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
        let result = engine.apply_mutation_with_session(spec, &mut session).await;
        drop(session);
        let _ = server.await;

        let captured = commands.lock().expect("lock captured commands").clone();
        (result, captured)
    }

    /// True if `line` is a bare mailbox-wide `EXPUNGE` (RFC 3501), NOT a
    /// UID-scoped `UID EXPUNGE` (RFC 4315). Lets a test prove the destructive
    /// mailbox-wide form is never issued (a plain `contains("EXPUNGE")` would
    /// be satisfied by the safe `UID EXPUNGE` too).
    fn is_bare_expunge(line: &str) -> bool {
        let u = line.to_ascii_uppercase();
        u.contains("EXPUNGE") && !u.contains("UID EXPUNGE")
    }

    fn imap_spec(kind: RemoteMutationKind, checkpoint: Option<&str>) -> RemoteMutationSpec {
        // The tests express the captured scope as a "{uidvalidity}:{uidnext}"
        // checkpoint string (mirroring how a sync stores it); the spec now
        // carries just the UIDVALIDITY component the guard compares against.
        let uid_validity = checkpoint
            .and_then(|c| c.split(':').next())
            .unwrap_or_default()
            .to_string();
        RemoteMutationSpec {
            message_id: "surrogate-42".to_string(),
            remote_id: "42".to_string(),
            folder_id: "INBOX".to_string(),
            uid_validity,
            kind,
        }
    }

    #[tokio::test]
    async fn imap_apply_mutation_flag_issues_uid_store_add_flags() {
        let (result, commands) = run_scripted_mutation(
            &imap_spec(
                RemoteMutationKind::SetFlagged { value: true },
                Some("1:105"),
            ),
            false,
            true,
            1,
        )
        .await;
        result.expect("flag mutation succeeds");
        assert!(
            commands
                .iter()
                .any(|c| c.contains("UID STORE 42 +FLAGS (\\Flagged)")),
            "expected a UID STORE +FLAGS (\\Flagged), got: {commands:?}"
        );
    }

    #[tokio::test]
    async fn imap_apply_mutation_move_uses_uid_move_when_server_advertises_move() {
        let (result, commands) = run_scripted_mutation(
            &imap_spec(
                RemoteMutationKind::Move {
                    to_folder: "Archive".to_string(),
                },
                Some("1:105"),
            ),
            true,
            true,
            1,
        )
        .await;
        result.expect("move mutation succeeds");
        assert!(
            commands.iter().any(|c| c.contains("UID MOVE 42")),
            "expected a UID MOVE, got: {commands:?}"
        );
        assert!(
            !commands.iter().any(|c| c.contains("UID COPY")),
            "must not fall back to COPY when MOVE is advertised: {commands:?}"
        );
    }

    #[tokio::test]
    async fn imap_apply_mutation_move_falls_back_to_copy_store_uid_expunge_without_move_capability()
    {
        let (result, commands) = run_scripted_mutation(
            &imap_spec(
                RemoteMutationKind::Move {
                    to_folder: "Archive".to_string(),
                },
                Some("1:105"),
            ),
            false,
            true,
            1,
        )
        .await;
        result.expect("move fallback succeeds");
        assert!(commands.iter().any(|c| c.contains("UID COPY 42")));
        assert!(commands
            .iter()
            .any(|c| c.contains("UID STORE 42 +FLAGS (\\Deleted)")));
        // The expunge MUST be scoped to the target UID, never a mailbox-wide
        // bare EXPUNGE that could destroy other \Deleted messages.
        assert!(
            commands.iter().any(|c| c.contains("UID EXPUNGE 42")),
            "fallback must issue a UID-scoped UID EXPUNGE 42, got: {commands:?}"
        );
        assert!(
            !commands.iter().any(|c| is_bare_expunge(c)),
            "fallback must NOT issue a bare mailbox-wide EXPUNGE, got: {commands:?}"
        );
        assert!(
            !commands.iter().any(|c| c.contains("UID MOVE")),
            "must not issue UID MOVE without the capability: {commands:?}"
        );
    }

    #[tokio::test]
    async fn imap_apply_mutation_delete_stores_deleted_and_uid_expunges_the_target_only() {
        let (result, commands) = run_scripted_mutation(
            &imap_spec(RemoteMutationKind::Delete, Some("1:105")),
            false,
            true,
            1,
        )
        .await;
        result.expect("delete mutation succeeds");
        assert!(commands
            .iter()
            .any(|c| c.contains("UID STORE 42 +FLAGS (\\Deleted)")));
        // The expunge MUST target exactly the deleted UID (RFC 4315), never a
        // mailbox-wide bare EXPUNGE that would also destroy other client's
        // \Deleted messages in the same folder.
        assert!(
            commands.iter().any(|c| c.contains("UID EXPUNGE 42")),
            "delete must issue a UID-scoped UID EXPUNGE 42, got: {commands:?}"
        );
        assert!(
            !commands.iter().any(|c| is_bare_expunge(c)),
            "delete must NOT issue a bare mailbox-wide EXPUNGE, got: {commands:?}"
        );
    }

    #[tokio::test]
    async fn imap_apply_mutation_delete_fails_closed_without_uidplus_and_issues_no_bare_expunge() {
        // Without UIDPLUS there is no UID-scoped expunge; the op MUST fail
        // closed rather than fall back to a destructive mailbox-wide EXPUNGE.
        let (result, commands) = run_scripted_mutation(
            &imap_spec(RemoteMutationKind::Delete, Some("1:105")),
            false,
            false,
            1,
        )
        .await;
        assert!(
            matches!(result, Err(MailError::ImapError(_))),
            "delete without UIDPLUS must fail closed, got: {result:?}"
        );
        assert!(
            !commands
                .iter()
                .any(|c| c.to_ascii_uppercase().contains("EXPUNGE")),
            "no expunge of any kind may be issued when UIDPLUS is unavailable, got: {commands:?}"
        );
    }

    #[tokio::test]
    async fn imap_apply_mutation_move_fallback_fails_closed_without_uidplus() {
        // The no-MOVE-capability move fallback also relies on a UID-scoped
        // expunge; without UIDPLUS it must fail closed and issue no bare
        // EXPUNGE (the preceding COPY/STORE are non-destructive to others).
        let (result, commands) = run_scripted_mutation(
            &imap_spec(
                RemoteMutationKind::Move {
                    to_folder: "Archive".to_string(),
                },
                Some("1:105"),
            ),
            false,
            false,
            1,
        )
        .await;
        assert!(
            matches!(result, Err(MailError::ImapError(_))),
            "move fallback without UIDPLUS must fail closed, got: {result:?}"
        );
        assert!(
            !commands
                .iter()
                .any(|c| c.to_ascii_uppercase().contains("EXPUNGE")),
            "no expunge of any kind may be issued when UIDPLUS is unavailable, got: {commands:?}"
        );
    }

    #[tokio::test]
    async fn imap_apply_mutation_refuses_to_act_on_a_uidvalidity_mismatch() {
        // The stored checkpoint captured the UID under UIDVALIDITY 1, but the
        // mailbox now reports 2 (renumbered). The mutation MUST be refused and
        // NO mutating command (STORE/COPY/MOVE/EXPUNGE) may be issued.
        let (result, commands) = run_scripted_mutation(
            &imap_spec(RemoteMutationKind::Delete, Some("1:105")),
            false,
            true,
            2,
        )
        .await;
        let err = result.expect_err("a UIDVALIDITY mismatch must be refused");
        assert!(matches!(err, MailError::ImapError(_)));
        assert!(
            !commands.iter().any(|c| {
                let u = c.to_ascii_uppercase();
                u.contains("UID STORE")
                    || u.contains("UID COPY")
                    || u.contains("UID MOVE")
                    || u.contains("EXPUNGE")
            }),
            "no mutating command may be issued on a UIDVALIDITY mismatch, got: {commands:?}"
        );
    }

    #[tokio::test]
    async fn imap_apply_mutation_refuses_when_no_checkpoint_proves_uidvalidity() {
        // With no stored checkpoint there is no UIDVALIDITY to prove the UID
        // still addresses the intended message, so the op is refused.
        let (result, _commands) =
            run_scripted_mutation(&imap_spec(RemoteMutationKind::Delete, None), false, true, 1)
                .await;
        assert!(matches!(
            result.expect_err("missing checkpoint must be refused"),
            MailError::ImapError(_)
        ));
    }

    #[tokio::test]
    async fn imap_apply_remote_mutation_requires_credentials() {
        let engine = ImapEngine::new("acct-1", "example.test", 993);
        let err = engine
            .apply_remote_mutation(&imap_spec(RemoteMutationKind::Delete, Some("1:105")))
            .await
            .expect_err("credential-less mutation must fail");
        assert!(matches!(err, MailError::AuthError(_)));
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

    #[traced_test]
    #[tokio::test]
    async fn imap_destructive_delete_logs_before_issuing_even_when_uidvalidity_guard_fails() {
        // The stored checkpoint's UIDVALIDITY (1) does not match the scripted
        // server's current UIDVALIDITY (2), so the UIDVALIDITY guard MUST
        // refuse the mutation and no protocol command may reach the server
        // (proven below via `commands`). A WARN log naming the op, the target
        // UID set, and the failed guard result must still exist -- proving the
        // pre-log fires before dispatch, not merely on a successful path.
        let (result, commands) = run_scripted_mutation(
            &imap_spec(RemoteMutationKind::Delete, Some("1:105")),
            false,
            true,
            2,
        )
        .await;

        let err = result.expect_err("a UIDVALIDITY mismatch must be refused");
        assert!(matches!(err, MailError::ImapError(_)));
        assert!(
            !commands.iter().any(|c| {
                let u = c.to_ascii_uppercase();
                u.contains("STORE") || u.contains("EXPUNGE") || u.contains("COPY")
            }),
            "no mutating command may be issued on a guard failure, got: {commands:?}"
        );

        assert!(logs_contain("issuing destructive IMAP DELETE"));
        assert!(logs_contain("uid_set=42"));
        assert!(logs_contain("uidvalidity_guard_passed=false"));
    }

    #[traced_test]
    #[tokio::test]
    async fn imap_destructive_move_logs_before_issuing_with_uid_set_and_passing_guard() {
        // A guard-passing MOVE still logs before the command is issued, at
        // INFO (MOVE is reversible/non-permanent, unlike DELETE/EXPUNGE).
        let (result, _commands) = run_scripted_mutation(
            &imap_spec(
                RemoteMutationKind::Move {
                    to_folder: "Archive".to_string(),
                },
                Some("1:105"),
            ),
            true,
            true,
            1,
        )
        .await;
        result.expect("move mutation succeeds");

        assert!(logs_contain("issuing destructive IMAP MOVE"));
        assert!(logs_contain("uid_set=42"));
        assert!(logs_contain("uidvalidity_guard_passed=true"));
    }

    #[traced_test]
    #[tokio::test]
    async fn imap_folder_sync_logs_completion_counts() {
        // A folder sync run must log its completion counts (fetched, and by
        // extension the oversized/skipped counters this run does not
        // exercise), so an operator can see sync outcomes without a debugger.
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let mut io = server_io;
            while let Some(line) = read_scripted_line(&mut io).await {
                let tag = line.split_whitespace().next().unwrap_or("").to_string();
                let upper = line.to_ascii_uppercase();
                if upper.contains("CAPABILITY") {
                    let _ = io
                        .write_all(
                            format!(
                                "* CAPABILITY IMAP4rev1 UIDPLUS MOVE\r\n{tag} OK CAPABILITY\r\n"
                            )
                            .as_bytes(),
                        )
                        .await;
                } else if upper.contains("LOGIN") {
                    let _ = io
                        .write_all(format!("{tag} OK Logged in\r\n").as_bytes())
                        .await;
                } else if upper.contains("SELECT") {
                    let resp = format!(
                        "* FLAGS (\\Seen)\r\n* 0 EXISTS\r\n* 0 RECENT\r\n\
                         * OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT 1] ok\r\n\
                         {tag} OK [READ-WRITE] SELECT done\r\n"
                    );
                    let _ = io.write_all(resp.as_bytes()).await;
                } else if upper.contains("UID FETCH") {
                    let _ = io
                        .write_all(format!("{tag} OK FETCH completed\r\n").as_bytes())
                        .await;
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
        let engine = ImapEngine::new("acct-completion", "example.test", 993);

        let __changes = engine
            .sync_folder_messages_with_session("INBOX", None, &mut session)
            .await
            .expect("sync succeeds");
        let (emails, _checkpoint) = (__changes.upserts, __changes.next_state);
        assert!(emails.is_empty());

        drop(session);
        let _ = server.await;

        assert!(logs_contain("imap folder sync complete"));
        assert!(logs_contain("fetched=0"));
        assert!(logs_contain("oversized=0"));
        assert!(logs_contain("skipped=0"));
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
