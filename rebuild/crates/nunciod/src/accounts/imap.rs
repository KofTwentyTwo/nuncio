use nuncio_engine::{accounts::AccountError, domain::imap_account as domain};
use nuncio_proto::v2;
use tonic::Status;
pub(super) fn config(value: v2::ImapAccountConfig) -> Result<domain::ImapAccountConfig, Status> {
    Ok(domain::ImapAccountConfig {
        address: value.address,
        imap: endpoint(value.imap.ok_or_else(invalid)?)?,
        smtp: endpoint(value.smtp.ok_or_else(invalid)?)?,
        sent_policy: match v2::SentPolicy::try_from(value.sent_policy) {
            Ok(v2::SentPolicy::Server) => domain::SentPolicy::Server,
            Ok(v2::SentPolicy::ClientAppend) => domain::SentPolicy::ClientAppend,
            _ => return Err(invalid()),
        },
        sent_folder: value.sent_folder,
        archive_folder: value.archive_folder,
        trash_folder: value.trash_folder,
        trusted_ca_pem: value.trusted_ca_pem,
    })
}
fn endpoint(value: v2::MailEndpoint) -> Result<domain::MailEndpoint, Status> {
    Ok(domain::MailEndpoint {
        host: value.host,
        port: u16::try_from(value.port).map_err(|_| invalid())?,
        tls: match v2::MailTls::try_from(value.tls) {
            Ok(v2::MailTls::Implicit) => domain::MailTls::Implicit,
            Ok(v2::MailTls::StartTls) => domain::MailTls::StartTls,
            _ => return Err(invalid()),
        },
        username: value.username,
    })
}
fn invalid() -> Status {
    super::error(AccountError::Invalid)
}
pub(super) fn capabilities(v: domain::ImapCapabilities) -> v2::ImapCapabilities {
    v2::ImapCapabilities {
        move_messages: v.move_messages,
        uidplus: v.uidplus,
        condstore: v.condstore,
        qresync: v.qresync,
        idle: v.idle,
        smtp_utf8: v.smtp_utf8,
        eight_bit_mime: v.eight_bit_mime,
    }
}
pub(super) fn configuration(v: domain::ImapAccountConfig) -> v2::ImapAccountConfig {
    v2::ImapAccountConfig {
        address: v.address,
        imap: Some(public_endpoint(v.imap)),
        smtp: Some(public_endpoint(v.smtp)),
        sent_policy: match v.sent_policy {
            domain::SentPolicy::Server => v2::SentPolicy::Server,
            domain::SentPolicy::ClientAppend => v2::SentPolicy::ClientAppend,
        }
        .into(),
        sent_folder: v.sent_folder,
        archive_folder: v.archive_folder,
        trash_folder: v.trash_folder,
        trusted_ca_pem: v.trusted_ca_pem,
    }
}
fn public_endpoint(v: domain::MailEndpoint) -> v2::MailEndpoint {
    v2::MailEndpoint {
        host: v.host,
        port: u32::from(v.port),
        tls: match v.tls {
            domain::MailTls::Implicit => v2::MailTls::Implicit,
            domain::MailTls::StartTls => v2::MailTls::StartTls,
        }
        .into(),
        username: v.username,
    }
}
