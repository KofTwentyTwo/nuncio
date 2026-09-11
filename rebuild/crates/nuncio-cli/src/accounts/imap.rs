use crate::output::AppError;
use nuncio_proto::v2;
use serde::{Deserialize, Serialize};
use std::{
    io::{IsTerminal, Read},
    path::Path,
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    schema_version: u32,
    address: String,
    imap: Endpoint,
    smtp: Endpoint,
    sent_policy: SentPolicy,
    sent_folder: String,
    archive_folder: Option<String>,
    trash_folder: Option<String>,
    trusted_ca_pem: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    host: String,
    port: u16,
    tls: Tls,
    username: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Tls {
    Implicit,
    StartTls,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SentPolicy {
    Server,
    ClientAppend,
}
#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct Credentials {
    imap_password: String,
    smtp_password: String,
}

pub(super) fn configuration(path: &Path) -> Result<v2::ImapAccountConfig, AppError> {
    let file = std::fs::File::open(path).map_err(|_| AppError::invalid())?;
    let meta = file.metadata().map_err(|_| AppError::invalid())?;
    if !meta.is_file() || meta.len() > 256 * 1024 {
        return Err(AppError::invalid());
    }
    let mut bytes = Vec::new();
    file.take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::invalid())?;
    decode_config(&bytes)
}
fn decode_config(bytes: &[u8]) -> Result<v2::ImapAccountConfig, AppError> {
    if bytes.len() > 256 * 1024 {
        return Err(AppError::invalid());
    }
    let config: Configuration = serde_json::from_slice(bytes).map_err(|_| AppError::invalid())?;
    if config.schema_version != 1 {
        return Err(AppError::invalid());
    }
    Ok(v2::ImapAccountConfig {
        address: config.address,
        imap: Some(endpoint(config.imap)),
        smtp: Some(endpoint(config.smtp)),
        sent_policy: match config.sent_policy {
            SentPolicy::Server => v2::SentPolicy::Server,
            SentPolicy::ClientAppend => v2::SentPolicy::ClientAppend,
        }
        .into(),
        sent_folder: config.sent_folder,
        archive_folder: config.archive_folder,
        trash_folder: config.trash_folder,
        trusted_ca_pem: config.trusted_ca_pem,
    })
}
fn endpoint(e: Endpoint) -> v2::MailEndpoint {
    v2::MailEndpoint {
        host: e.host,
        port: e.port.into(),
        tls: match e.tls {
            Tls::Implicit => v2::MailTls::Implicit,
            Tls::StartTls => v2::MailTls::StartTls,
        }
        .into(),
        username: e.username,
    }
}
pub(super) fn credentials() -> Result<v2::ImapCredentials, AppError> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(AppError {code:"credential_input",message:"Pipe a credential JSON object from a secure source; interactive terminal input is refused to prevent password echo",exit:2,sync_run:None,operation: None, recovery: None});
    }
    let mut bytes = Zeroizing::new(Vec::new());
    stdin
        .lock()
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::invalid())?;
    if bytes.len() > 16384 {
        return Err(AppError::invalid());
    }
    let mut value: Credentials = serde_json::from_slice(&bytes).map_err(|_| AppError::invalid())?;
    Ok(v2::ImapCredentials {
        imap_password: std::mem::take(&mut value.imap_password),
        smtp_password: std::mem::take(&mut value.smtp_password),
    })
}
pub(super) fn public_config(v: v2::ImapAccountConfig) -> Result<serde_json::Value, AppError> {
    let endpoint = |v: v2::MailEndpoint| -> Result<Endpoint, AppError> {
        Ok(Endpoint {
            host: v.host,
            port: u16::try_from(v.port).map_err(|_| AppError::invalid())?,
            tls: match v2::MailTls::try_from(v.tls) {
                Ok(v2::MailTls::Implicit) => Tls::Implicit,
                Ok(v2::MailTls::StartTls) => Tls::StartTls,
                _ => return Err(AppError::invalid()),
            },
            username: v.username,
        })
    };
    let config = Configuration {
        schema_version: 1,
        address: v.address,
        imap: endpoint(v.imap.ok_or_else(AppError::invalid)?)?,
        smtp: endpoint(v.smtp.ok_or_else(AppError::invalid)?)?,
        sent_policy: match v2::SentPolicy::try_from(v.sent_policy) {
            Ok(v2::SentPolicy::Server) => SentPolicy::Server,
            Ok(v2::SentPolicy::ClientAppend) => SentPolicy::ClientAppend,
            _ => return Err(AppError::invalid()),
        },
        sent_folder: v.sent_folder,
        archive_folder: v.archive_folder,
        trash_folder: v.trash_folder,
        trusted_ca_pem: v.trusted_ca_pem,
    };
    serde_json::to_value(config).map_err(|_| AppError::invalid())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_rejects_credential_fields_and_unknown_transport() {
        let config = serde_json::json!({"schema_version":1,"address":"alpha@example.test","imap":{"host":"localhost","port":993,"tls":"implicit","username":"alpha@example.test"},"smtp":{"host":"localhost","port":587,"tls":"start_tls","username":"alpha@example.test"},"sent_policy":"client_append","sent_folder":"Sent"});
        assert!(decode_config(config.to_string().as_bytes()).is_ok());
        for (key, value) in [
            ("password", serde_json::json!("test-only-canary")),
            ("schema_version", serde_json::json!(2)),
        ] {
            let mut bad = config.clone();
            bad[key] = value;
            assert!(decode_config(bad.to_string().as_bytes()).is_err());
        }
        let mut bad = config;
        bad["imap"]["tls"] = serde_json::json!("none");
        assert!(decode_config(bad.to_string().as_bytes()).is_err());
    }
}
