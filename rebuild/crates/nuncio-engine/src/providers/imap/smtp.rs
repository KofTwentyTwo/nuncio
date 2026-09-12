use super::{wire::Wire, MailError};
use crate::domain::imap_account::{MailEndpoint, MailTls};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use lettre::transport::smtp::response::Response;
use std::{collections::BTreeSet, sync::Arc};
use tokio::io::AsyncWriteExt;
use tokio_rustls::rustls;
use zeroize::Zeroizing;

// Keep framing bounded before parsing. lettre's connection reads an entire line
// before checking its length; that cannot enforce our untrusted-input budget.
async fn response(wire: &mut Wire) -> Result<Response, MailError> {
    let mut bytes = Vec::new();
    let mut code = None;
    for _ in 0..128 {
        let line = wire.line(2048).await?;
        if line.len() < 5 || !line[..3].iter().all(u8::is_ascii_digit) {
            return Err(MailError::Protocol);
        }
        let current = [line[0], line[1], line[2]];
        if code.is_some_and(|c| c != current) {
            return Err(MailError::Protocol);
        }
        code = Some(current);
        let done = line[3] == b' ' || line[3] == b'\r';
        if !done && line[3] != b'-' {
            return Err(MailError::Protocol);
        }
        bytes.extend_from_slice(&line);
        if bytes.len() > 65536 {
            return Err(MailError::Protocol);
        }
        if done {
            return std::str::from_utf8(&bytes)
                .map_err(|_| MailError::Protocol)?
                .parse()
                .map_err(|_| MailError::Protocol);
        }
    }
    Err(MailError::Protocol)
}
async fn command(wire: &mut Wire, bytes: &[u8]) -> Result<Response, MailError> {
    let _request = wire
        .resources
        .request()
        .await
        .map_err(|_| MailError::Unavailable)?;
    wire.write_all(bytes)
        .await
        .map_err(|_| MailError::Unavailable)?;
    response(wire).await
}
async fn ehlo(wire: &mut Wire) -> Result<BTreeSet<String>, MailError> {
    let reply = command(wire, b"EHLO [127.0.0.1]\r\n").await?;
    if !reply.has_code(250) {
        return Err(MailError::Unsupported);
    }
    Ok(reply
        .message()
        .skip(1)
        .map(|s| s.to_ascii_uppercase())
        .collect())
}
pub(crate) struct Session {
    wire: Wire,
    caps: BTreeSet<String>,
}
pub(super) async fn authenticate(
    endpoint: &MailEndpoint,
    password: &str,
    trust: Arc<rustls::ClientConfig>,
    resources: Arc<crate::resources::Resources>,
) -> Result<(bool, bool), MailError> {
    let session = connect(endpoint, password, trust, resources).await?;
    Ok((
        session.caps.contains("SMTPUTF8"),
        session.caps.contains("8BITMIME"),
    ))
}
pub(super) async fn connect(
    endpoint: &MailEndpoint,
    password: &str,
    trust: Arc<rustls::ClientConfig>,
    resources: Arc<crate::resources::Resources>,
) -> Result<Session, MailError> {
    let handshake = resources
        .request()
        .await
        .map_err(|_| MailError::Unavailable)?;
    let mut wire = Wire::connect(endpoint, resources.clone()).await?;
    if endpoint.tls == MailTls::Implicit {
        wire = wire.encrypt(&endpoint.host, trust.clone()).await?;
    }
    if !response(&mut wire).await?.has_code(220) {
        return Err(MailError::Protocol);
    }
    drop(handshake);
    let mut caps = ehlo(&mut wire).await?;
    if endpoint.tls == MailTls::StartTls {
        if !caps.contains("STARTTLS") {
            return Err(MailError::Tls);
        }
        if !command(&mut wire, b"STARTTLS\r\n").await?.has_code(220) {
            return Err(MailError::Tls);
        }
        {
            let _handshake = resources
                .request()
                .await
                .map_err(|_| MailError::Unavailable)?;
            wire = wire.encrypt(&endpoint.host, trust).await?;
        }
        caps = ehlo(&mut wire).await?;
    }
    let mechanisms: BTreeSet<_> = caps
        .iter()
        .filter_map(|s| s.strip_prefix("AUTH "))
        .flat_map(str::split_ascii_whitespace)
        .collect();
    let reply = if mechanisms.contains("PLAIN") {
        let plain = Zeroizing::new(format!("\0{}\0{password}", endpoint.username));
        let encoded = Zeroizing::new(STANDARD.encode(plain.as_bytes()));
        let request = Zeroizing::new(format!("AUTH PLAIN {}\r\n", encoded.as_str()));
        let reply = command(&mut wire, request.as_bytes()).await?;
        if reply.has_code(334) {
            let challenge_response = Zeroizing::new(format!("{}\r\n", encoded.as_str()));
            command(&mut wire, challenge_response.as_bytes()).await?
        } else {
            reply
        }
    } else if mechanisms.contains("LOGIN") {
        login(&mut wire, &endpoint.username, password).await?
    } else {
        return Err(MailError::Unsupported);
    };
    require_auth(&reply, 235)?;
    Ok(Session { wire, caps })
}

async fn login(wire: &mut Wire, username: &str, password: &str) -> Result<Response, MailError> {
    require_auth(&command(wire, b"AUTH LOGIN\r\n").await?, 334)?;
    let user = Zeroizing::new(format!("{}\r\n", STANDARD.encode(username)));
    require_auth(&command(wire, user.as_bytes()).await?, 334)?;
    let pass = Zeroizing::new(format!("{}\r\n", STANDARD.encode(password)));
    let reply = command(wire, pass.as_bytes()).await?;
    require_auth(&reply, 235)?;
    Ok(reply)
}

fn require_auth(response: &Response, expected: u16) -> Result<(), MailError> {
    let code = u16::from(response.code());
    if code == expected {
        Ok(())
    } else if (400..500).contains(&code) {
        Err(MailError::Unavailable)
    } else if matches!(code, 534 | 535 | 538) {
        Err(MailError::Authorization)
    } else {
        Err(MailError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
    };
    async fn server(replies: Vec<&'static [u8]>) -> (Wire, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let peer = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut peer = BufReader::new(socket);
            let mut commands = Vec::new();
            for reply in replies {
                let mut command = Vec::new();
                peer.read_until(b'\n', &mut command).await.unwrap();
                commands.push(command);
                peer.get_mut().write_all(reply).await.unwrap();
            }
            commands
        });
        (Wire::test(socket), peer)
    }
    #[tokio::test]
    async fn login_temporary_rejection_at_each_challenge_keeps_credentials() {
        for replies in [
            vec![b"451 try later\r\n".as_slice()],
            vec![b"334 VXNlcm5hbWU6\r\n", b"454 temporarily unavailable\r\n"],
            vec![
                b"334 VXNlcm5hbWU6\r\n",
                b"334 UGFzc3dvcmQ6\r\n",
                b"451 try later\r\n",
            ],
        ] {
            let expected = replies.len();
            let (mut wire, peer) = server(replies).await;
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                login(&mut wire, "unit-user", "unit-password"),
            )
            .await
            .unwrap();
            assert!(matches!(result, Err(MailError::Unavailable)));
            let commands = peer.await.unwrap();
            assert_eq!(commands.len(), expected);
            assert_eq!(commands[0], b"AUTH LOGIN\r\n");
        }
    }
    #[tokio::test]
    async fn bounded_reply_parser_rejects_unterminated_and_mixed_code_responses() {
        for reply in [
            b"250-first line\r\n550 wrong code\r\n".as_slice(),
            b"250 invalid\n",
        ] {
            let (mut wire, peer) = server(vec![reply]).await;
            assert!(matches!(
                command(&mut wire, b"EHLO [127.0.0.1]\r\n").await,
                Err(MailError::Protocol)
            ));
            peer.await.unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut wire = Wire::test(
            TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap(),
        );
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(&vec![b'X'; 4096]).await.unwrap();
            // Keep the socket open: receiving the configured bound must suffice.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        });
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), response(&mut wire))
                .await
                .unwrap(),
            Err(MailError::Protocol)
        ));
        peer.abort();
        let _ = peer.await;
    }
}

#[cfg(test)]
mod submission_tests;

pub(crate) mod submission;
