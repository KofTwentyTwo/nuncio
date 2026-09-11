use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, net::IpAddr};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, thiserror::Error)]
#[error("Invalid IMAP/SMTP account configuration or credential input")]
pub struct ImapConfigError;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MailTls {
    Implicit,
    StartTls,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SentPolicy {
    Server,
    ClientAppend,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MailEndpoint {
    pub host: String,
    pub port: u16,
    pub tls: MailTls,
    pub username: String,
}

impl MailEndpoint {
    fn canonicalize(&mut self) -> Result<(), ImapConfigError> {
        if self.port == 0
            || self.username.is_empty()
            || self.username.len() > 1024
            || self.username.chars().any(char::is_control)
            || self.host.len() > 1024
            || self.host.chars().any(char::is_control)
        {
            return Err(ImapConfigError);
        }
        if let Ok(ip) = self.host.parse::<IpAddr>() {
            self.host = ip.to_string();
            return Ok(());
        }
        self.host = match url::Host::parse(&self.host).map_err(|_| ImapConfigError)? {
            url::Host::Ipv4(ip) => ip.to_string(),
            url::Host::Ipv6(ip) => ip.to_string(),
            url::Host::Domain(domain) => {
                let domain = domain.strip_suffix('.').unwrap_or(&domain);
                if domain.len() > 253
                    || domain.split('.').any(|label| {
                        label.is_empty()
                            || label.len() > 63
                            || label.starts_with('-')
                            || label.ends_with('-')
                            || !label
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    })
                {
                    return Err(ImapConfigError);
                }
                domain.to_ascii_lowercase()
            }
        };
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImapAccountConfig {
    pub address: String,
    pub imap: MailEndpoint,
    pub smtp: MailEndpoint,
    pub sent_policy: SentPolicy,
    pub sent_folder: String,
    pub archive_folder: Option<String>,
    pub trash_folder: Option<String>,
    pub trusted_ca_pem: Option<String>,
}

impl ImapAccountConfig {
    pub fn canonicalized(mut self) -> Result<Self, ImapConfigError> {
        super::drafts::Recipient {
            address: self.address.clone(),
            name: None,
        }
        .validate()
        .map_err(|_| ImapConfigError)?;
        self.imap.canonicalize()?;
        self.smtp.canonicalize()?;
        for folder in std::iter::once(&self.sent_folder)
            .chain(self.archive_folder.iter())
            .chain(self.trash_folder.iter())
        {
            validate_mailbox(folder)?;
        }
        if self
            .trusted_ca_pem
            .as_ref()
            .is_some_and(|pem| pem.is_empty() || pem.len() > 128 * 1024 || pem.contains('\0'))
        {
            return Err(ImapConfigError);
        }
        // X.509 parsing and hostname verification occur before authentication
        // or persistence, in the transport that consumes these trust roots.
        Ok(self)
    }

    pub fn identity(&self) -> Result<String, ImapConfigError> {
        let canonical = self.clone().canonicalized()?;
        // IMAP has no Google-style immutable subject. Pin the authenticated
        // principal to its canonical endpoint and exact (case-sensitive) user.
        let encoded = serde_json::to_vec(&("imap-principal-v1", &canonical.imap))
            .map_err(|_| ImapConfigError)?;
        Ok(format!("imap:{}", hex::encode(Sha256::digest(encoded))))
    }
}

pub fn validate_mailbox(name: &str) -> Result<(), ImapConfigError> {
    if name.is_empty() || name.len() > 2048 || name.chars().any(char::is_control) {
        Err(ImapConfigError)
    } else {
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct ImapCredentials {
    pub imap_password: String,
    pub smtp_password: String,
}

impl ImapCredentials {
    pub fn validate(&self) -> Result<(), ImapConfigError> {
        if [&self.imap_password, &self.smtp_password]
            .iter()
            .any(|value| value.is_empty() || value.len() > 4096 || value.contains('\0'))
        {
            Err(ImapConfigError)
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for ImapCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ImapCredentials([redacted])")
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapCapabilities {
    pub move_messages: bool,
    pub uidplus: bool,
    pub condstore: bool,
    pub qresync: bool,
    pub idle: bool,
    pub smtp_utf8: bool,
    pub eight_bit_mime: bool,
}
