use super::MailError;
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    net::TcpStream,
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{pem::PemObject, CertificateDer, ServerName},
    },
    TlsConnector,
};

enum Transport {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}
pub(super) struct Wire {
    transport: Transport,
    pub resources: Arc<crate::resources::Resources>,
}
impl std::fmt::Debug for Wire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MailWire([redacted])")
    }
}
impl Wire {
    #[cfg(test)]
    pub fn test(tcp: TcpStream) -> Self {
        Self {
            transport: Transport::Plain(tcp),
            resources: crate::resources::Resources::new(),
        }
    }

    pub async fn connect(
        endpoint: &crate::domain::imap_account::MailEndpoint,
        resources: Arc<crate::resources::Resources>,
    ) -> Result<Self, MailError> {
        let tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(|_| MailError::Unavailable)?;
        tcp.set_nodelay(true).map_err(|_| MailError::Unavailable)?;
        Ok(Self {
            transport: Transport::Plain(tcp),
            resources,
        })
    }
    pub async fn encrypt(
        self,
        host: &str,
        roots: Arc<rustls::ClientConfig>,
    ) -> Result<Self, MailError> {
        let Transport::Plain(tcp) = self.transport else {
            return Err(MailError::Protocol);
        };
        let name = ServerName::try_from(host.to_owned()).map_err(|_| MailError::Invalid)?;
        TlsConnector::from(roots)
            .connect(name, tcp)
            .await
            .map(|tls| Self {
                transport: Transport::Tls(Box::new(tls)),
                resources: self.resources,
            })
            .map_err(|_| MailError::Tls)
    }
    // Reading only to CRLF avoids carrying pre-STARTTLS bytes across the trust boundary.
    pub async fn line(&mut self, limit: usize) -> Result<Vec<u8>, MailError> {
        let mut line = Vec::new();
        while line.len() < limit {
            let byte = self.read_u8().await.map_err(|_| MailError::Unavailable)?;
            line.push(byte);
            if byte == b'\n' {
                return if line.ends_with(b"\r\n") {
                    Ok(line)
                } else {
                    Err(MailError::Protocol)
                };
            }
        }
        Err(MailError::Protocol)
    }
}
impl AsyncRead for Wire {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = b.filled().len();
        let result = match &mut self.transport {
            Transport::Plain(s) => Pin::new(s).poll_read(cx, b),
            Transport::Tls(s) => Pin::new(&mut **s).poll_read(cx, b),
        };
        self.resources
            .received(b.filled().len().saturating_sub(before));
        result
    }
}
impl AsyncWrite for Wire {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.transport {
            Transport::Plain(s) => Pin::new(s).poll_write(cx, b),
            Transport::Tls(s) => Pin::new(&mut **s).poll_write(cx, b),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.transport {
            Transport::Plain(s) => Pin::new(s).poll_flush(cx),
            Transport::Tls(s) => Pin::new(&mut **s).poll_flush(cx),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.transport {
            Transport::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Transport::Tls(s) => Pin::new(&mut **s).poll_shutdown(cx),
        }
    }
}

pub(super) fn trust(pem: Option<&str>) -> Result<Arc<rustls::ClientConfig>, MailError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = pem {
        let certs = CertificateDer::pem_slice_iter(pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MailError::Invalid)?;
        if certs.is_empty() {
            return Err(MailError::Invalid);
        }
        for cert in certs {
            roots.add(cert).map_err(|_| MailError::Invalid)?;
        }
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| MailError::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth(),
    ))
}

/// Validate complete bounded lines and literal sizes before async-imap's parser
/// sees a size announcement and allocates its buffer. Authentication permits no literals.
pub(super) struct ImapWire {
    wire: Wire,
    pending: Vec<u8>,
    offset: usize,
    remaining: usize,
    literal_left: usize,
    literal_limit: usize,
    auth_rejected: Arc<std::sync::atomic::AtomicBool>,
}
impl std::fmt::Debug for ImapWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImapImapWire([redacted])")
    }
}
impl ImapWire {
    pub fn resources(&self) -> Arc<crate::resources::Resources> {
        self.wire.resources.clone()
    }

    pub fn set_limits(&mut self, total: usize, literal: usize) -> io::Result<()> {
        if total == 0
            || total > 70 * 1024 * 1024
            || literal > 64 * 1024 * 1024
            || literal > total
            || !self.pending.is_empty()
            || self.literal_left != 0
        {
            return Err(io::Error::other(
                "Invalid IMAP read limits or unfinished response",
            ));
        }
        self.remaining = total;
        self.literal_limit = literal;
        Ok(())
    }
    pub fn new(
        wire: Wire,
        auth_rejected: Arc<std::sync::atomic::AtomicBool>,
        total: usize,
        literal: usize,
    ) -> io::Result<Self> {
        let mut framed = Self {
            wire,
            pending: Vec::new(),
            offset: 0,
            remaining: 0,
            literal_left: 0,
            literal_limit: 0,
            auth_rejected,
        };
        framed.set_limits(total, literal)?;
        Ok(framed)
    }
}
impl AsyncRead for ImapWire {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            if self.pending.ends_with(b"\r\n") {
                if self.offset == 0 {
                    if let Some(size) = literal_size(&self.pending)? {
                        if size > self.literal_limit || size > self.remaining {
                            return Poll::Ready(Err(io::Error::other(
                                "IMAP literal exceeds read budget",
                            )));
                        }
                        self.literal_left = size;
                    }
                    let mut tokens = self.pending.split(|b| *b == b' ');
                    let tag = tokens.next();
                    let status = tokens.next();
                    let code = tokens.next();
                    if tag.is_some_and(|tag| tag != b"*")
                        && status.is_some_and(|s| s.eq_ignore_ascii_case(b"NO"))
                        && code.is_some_and(|c| {
                            [
                                b"[AUTHENTICATIONFAILED]".as_slice(),
                                b"[AUTHORIZATIONFAILED]",
                                b"[EXPIRED]",
                            ]
                            .iter()
                            .any(|known| c.eq_ignore_ascii_case(known))
                        })
                    {
                        self.auth_rejected
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                let count = out.remaining().min(self.pending.len() - self.offset);
                out.put_slice(&self.pending[self.offset..self.offset + count]);
                self.offset += count;
                if self.offset == self.pending.len() {
                    self.pending.clear();
                    self.offset = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if self.literal_left > 0 {
                let count = out.remaining().min(self.literal_left).min(self.remaining);
                if count == 0 {
                    return Poll::Ready(Err(io::Error::other("IMAP response exceeds read budget")));
                }
                let mut limited = ReadBuf::new(out.initialize_unfilled_to(count));
                let result = Pin::new(&mut self.wire).poll_read(cx, &mut limited);
                let read = limited.filled().len();
                match result {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Ready(Ok(())) if read == 0 => {
                        return Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)))
                    }
                    Poll::Ready(Ok(())) => {
                        self.literal_left -= read;
                        self.remaining -= read;
                        out.advance(read);
                        return Poll::Ready(Ok(()));
                    }
                }
            }
            if self.remaining == 0 || self.pending.len() >= 65536 {
                return Poll::Ready(Err(io::Error::other("IMAP framing exceeds read budget")));
            }
            let mut byte = [0];
            let mut buffer = ReadBuf::new(&mut byte);
            match Pin::new(&mut self.wire).poll_read(cx, &mut buffer) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {
                    if buffer.filled().is_empty() {
                        return Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)));
                    }
                    self.remaining -= 1;
                    self.pending.push(byte[0]);
                    if byte[0] == b'\n' && !self.pending.ends_with(b"\r\n") {
                        return Poll::Ready(Err(io::Error::other("Invalid IMAP framing")));
                    }
                }
            }
        }
    }
}
fn literal_size(line: &[u8]) -> io::Result<Option<usize>> {
    let Some(prefix) = line.strip_suffix(b"}\r\n") else {
        return Ok(None);
    };
    // A complete status response can legitimately end in text such as "{3}".
    if async_imap::imap_proto::parser::parse_response(line).is_ok() {
        return Ok(None);
    }
    let Some(offset) = prefix.iter().rposition(|b| *b == b'{') else {
        return Ok(None);
    };
    let digits = &prefix[offset + 1..];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(io::Error::other("Invalid IMAP literal length"));
    }
    let mut size = 0_usize;
    for digit in digits {
        size = size
            .checked_mul(10)
            .and_then(|n| n.checked_add(usize::from(digit - b'0')))
            .ok_or_else(|| io::Error::other("IMAP literal length overflow"))?;
    }
    Ok(Some(size))
}

impl AsyncWrite for ImapWire {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.wire).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.wire).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.wire).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use tokio::io::AsyncWriteExt;
    async fn raw_server(bytes: Vec<u8>) -> (ImapWire, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(&bytes).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        });
        (
            ImapWire::new(
                Wire::test(socket),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                256 * 1024,
                0,
            )
            .unwrap(),
            peer,
        )
    }
    #[tokio::test]
    async fn literal_framing_preserves_binary_bytes_and_checks_sizes_before_parser_allocation() {
        let raw = b"\0a\r\nb\xffz!";
        let mut bytes = b"* 1 FETCH (UID 7 BODY[] {8}\r\n".to_vec();
        bytes.extend_from_slice(raw);
        bytes.extend_from_slice(b")\r\n");
        let (mut wire, peer) = raw_server(bytes).await;
        wire.set_limits(65536, 8).unwrap();
        let mut client = async_imap::Client::new(wire);
        let value = tokio::time::timeout(std::time::Duration::from_secs(1), client.read_response())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            matches!(value.parsed(),async_imap::imap_proto::Response::Fetch(_,attrs) if attrs.iter().any(|a|matches!(a,async_imap::imap_proto::AttributeValue::BodySection{data:Some(bytes),..} if bytes.as_ref()==raw)))
        );
        peer.abort();
        let _ = peer.await;
        for announced in [9_u64, 512 * 1024 * 1024, u64::MAX] {
            let (mut wire, peer) =
                raw_server(format!("* 1 FETCH (UID 7 BODY[] {{{announced}}}\r\n").into_bytes())
                    .await;
            wire.set_limits(65536, 8).unwrap();
            let mut client = async_imap::Client::new(wire);
            assert!(tokio::time::timeout(
                std::time::Duration::from_secs(1),
                client.read_response()
            )
            .await
            .unwrap()
            .is_err());
            peer.abort();
            let _ = peer.await;
        }
    }
}
