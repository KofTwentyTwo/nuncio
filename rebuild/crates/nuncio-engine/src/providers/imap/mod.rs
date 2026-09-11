mod command;
pub(crate) mod flags;
mod read;
pub(crate) mod sent;
pub(crate) mod smtp;
pub(crate) mod sync;
pub(crate) mod transfers;
mod wire;
use crate::domain::imap_account::{ImapAccountConfig, ImapCapabilities, ImapCredentials, MailTls};
use async_imap::imap_proto::{Response, Status};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub(crate) enum MailError {
    #[error("Invalid IMAP/SMTP configuration")]
    Invalid,
    #[error("Mail server could not be reached within the request deadline")]
    Unavailable,
    #[error("Mail server TLS verification failed")]
    Tls,
    #[error("Mail server authentication failed")]
    Authorization,
    #[error("Mail server does not support the required protocol capability")]
    Unsupported,
    #[error("Mail server returned an invalid or oversized response")]
    Protocol,
}
impl From<MailError> for crate::accounts::AccountError {
    fn from(error: MailError) -> Self {
        use crate::accounts::AccountError;
        match error {
            MailError::Invalid => AccountError::Invalid,
            MailError::Unavailable => AccountError::Unavailable,
            MailError::Tls => AccountError::Tls,
            MailError::Authorization => AccountError::Authorization,
            MailError::Unsupported => AccountError::Unsupported,
            MailError::Protocol => AccountError::Provider,
        }
    }
}
pub(crate) async fn probe(
    config: &ImapAccountConfig,
    credentials: &ImapCredentials,
) -> Result<ImapCapabilities, MailError> {
    let trust = wire::trust(config.trusted_ca_pem.as_deref())?;
    tokio::time::timeout(
        Duration::from_secs(30),
        probe_inner(config, credentials, trust),
    )
    .await
    .map_err(|_| MailError::Unavailable)?
}
async fn probe_inner(
    config: &ImapAccountConfig,
    credentials: &ImapCredentials,
    trust: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
) -> Result<ImapCapabilities, MailError> {
    let mut connection = open_inner(config, credentials, trust.clone()).await?;
    drop(connection.session);
    let smtp = smtp::authenticate(&config.smtp, &credentials.smtp_password, trust).await?;
    connection.capabilities.smtp_utf8 = smtp.0;
    connection.capabilities.eight_bit_mime = smtp.1;
    Ok(connection.capabilities)
}

pub(crate) struct Connection {
    session: async_imap::Session<wire::ImapWire>,
    pub capabilities: ImapCapabilities,
}
pub(crate) async fn open(
    config: &ImapAccountConfig,
    credentials: &ImapCredentials,
) -> Result<Connection, MailError> {
    let trust = wire::trust(config.trusted_ca_pem.as_deref())?;
    tokio::time::timeout(
        Duration::from_secs(30),
        open_inner(config, credentials, trust),
    )
    .await
    .map_err(|_| MailError::Unavailable)?
}
async fn open_inner(
    config: &ImapAccountConfig,
    credentials: &ImapCredentials,
    trust: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
) -> Result<Connection, MailError> {
    let mut wire = wire::Wire::connect(&config.imap).await?;
    if config.imap.tls == MailTls::Implicit {
        wire = wire.encrypt(&config.imap.host, trust.clone()).await?;
    }
    let greeting = wire.line(65536).await?;
    let (rest, response) = async_imap::imap_proto::parser::parse_response(&greeting)
        .map_err(|_| MailError::Protocol)?;
    if !rest.is_empty()
        || !matches!(
            response,
            Response::Data {
                status: Status::Ok,
                ..
            }
        )
    {
        return Err(MailError::Protocol);
    }
    if config.imap.tls == MailTls::StartTls {
        wire.write_all(b"nuncio_tls STARTTLS\r\n")
            .await
            .map_err(|_| MailError::Unavailable)?;
        let response = wire.line(65536).await?;
        let (rest, response) = async_imap::imap_proto::parser::parse_response(&response)
            .map_err(|_| MailError::Protocol)?;
        if !rest.is_empty()
            || !matches!(response,Response::Done{ref tag,status:Status::Ok,..} if tag.0=="nuncio_tls")
        {
            return Err(MailError::Tls);
        }
        wire = wire.encrypt(&config.imap.host, trust.clone()).await?;
    }
    let rejected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let client = async_imap::Client::new(
        wire::ImapWire::new(wire, rejected.clone(), 256 * 1024, 0)
            .map_err(|_| MailError::Protocol)?,
    );
    let auth = Plain {
        bytes: Zeroizing::new(
            format!("\0{}\0{}", config.imap.username, credentials.imap_password).into_bytes(),
        ),
        used: false,
    };
    let session = client
        .authenticate("PLAIN", auth)
        .await
        .map_err(|(error, _)| match error {
            async_imap::error::Error::No(_)
                if rejected.load(std::sync::atomic::Ordering::Relaxed) =>
            {
                MailError::Authorization
            }
            async_imap::error::Error::No(_) | async_imap::error::Error::ConnectionLost => {
                MailError::Unavailable
            }
            async_imap::error::Error::Bad(_) => MailError::Unsupported,
            _ => MailError::Protocol,
        })?;
    let mut session = session;
    let capabilities = read_capabilities(&mut session).await?;
    Ok(Connection {
        session,
        capabilities,
    })
}

async fn read_capabilities(
    session: &mut async_imap::Session<wire::ImapWire>,
) -> Result<ImapCapabilities, MailError> {
    // Several async-imap collectors discard the terminal status. Preserve the
    // parser but require our own matching tagged OK before trusting any data.
    let responses = command::complete(session, "CAPABILITY", 256 * 1024, 0, |response| {
        Ok(match response {
            Response::Capabilities(caps) => Some(
                caps.iter()
                    .map(async_imap::types::Capability::from)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
    })
    .await?;
    let has = |name: &str| {
        responses.iter().flatten().any(|cap| match cap {
            async_imap::types::Capability::Imap4rev1 => name.eq_ignore_ascii_case("IMAP4REV1"),
            async_imap::types::Capability::Atom(value) => value.eq_ignore_ascii_case(name),
            _ => false,
        })
    };
    if !has("IMAP4REV1") {
        return Err(MailError::Unsupported);
    }
    let result = ImapCapabilities {
        move_messages: has("MOVE"),
        uidplus: has("UIDPLUS"),
        condstore: has("CONDSTORE"),
        qresync: has("QRESYNC"),
        idle: has("IDLE"),
        ..Default::default()
    };

    Ok(result)
}

struct Plain {
    bytes: Zeroizing<Vec<u8>>,
    used: bool,
}
impl async_imap::Authenticator for Plain {
    type Response = Zeroizing<Vec<u8>>;
    fn process(&mut self, _: &[u8]) -> Self::Response {
        if self.used {
            Zeroizing::new(Vec::new())
        } else {
            self.used = true;
            std::mem::take(&mut self.bytes)
        }
    }
}

pub(crate) async fn open_smtp(
    config: &ImapAccountConfig,
    credentials: &ImapCredentials,
) -> Result<smtp::Session, MailError> {
    let trust = wire::trust(config.trusted_ca_pem.as_deref())?;
    tokio::time::timeout(
        Duration::from_secs(30),
        smtp::connect(&config.smtp, &credentials.smtp_password, trust),
    )
    .await
    .map_err(|_| MailError::Unavailable)?
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        net::{TcpListener, TcpStream},
    };
    #[tokio::test]
    async fn partial_capabilities_require_matching_positive_completion() {
        for status in ["NO", "BAD", "WRONG_TAG", "DISCONNECT", "BYE", "OK"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let tcp = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let peer = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut peer = BufReader::new(tcp);
                let mut command = String::new();
                peer.read_line(&mut command).await.unwrap();
                let auth_tag = command.split_once(' ').unwrap().0.to_owned();
                peer.get_mut().write_all(b"+\r\n").await.unwrap();
                command.clear();
                peer.read_line(&mut command).await.unwrap();
                peer.get_mut()
                    .write_all(format!("{auth_tag} OK authenticated\r\n").as_bytes())
                    .await
                    .unwrap();
                command.clear();
                peer.read_line(&mut command).await.unwrap();
                let tag = command.split_once(' ').unwrap().0;
                let completion = match status {
                    "WRONG_TAG" => "unrelated OK complete\r\n".into(),
                    "DISCONNECT" => String::new(),
                    "BYE" => "* BYE server closed\r\n".into(),
                    status => format!("{tag} {status} complete\r\n"),
                };
                peer.get_mut()
                    .write_all(format!("* CAPABILITY IMAP4rev1 UIDPLUS\r\n{completion}").as_bytes())
                    .await
                    .unwrap();
            });
            let wire = wire::ImapWire::new(
                wire::Wire::Plain(tcp),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                256 * 1024,
                0,
            )
            .unwrap();
            let client = async_imap::Client::new(wire);
            let mut session = client
                .authenticate(
                    "PLAIN",
                    Plain {
                        bytes: Zeroizing::new(b"\0unit\0secret".to_vec()),
                        used: false,
                    },
                )
                .await
                .unwrap();
            let result =
                tokio::time::timeout(Duration::from_secs(2), read_capabilities(&mut session))
                    .await
                    .unwrap();
            if status == "OK" {
                assert!(result.unwrap().uidplus);
            } else {
                assert!(result.is_err());
            }
            peer.await.unwrap();
        }
    }
}
