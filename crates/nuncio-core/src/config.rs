//! Account configuration models and validation logic.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Protocol engine type supported for account synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountProtocol {
    /// JMAP (RFC 8620 / RFC 8621) protocol engine.
    Jmap,
    /// Legacy IMAP4rev1 + SMTP protocol engines.
    ImapSmtp,
    /// CalDAV (RFC 4791) calendar-collection protocol engine. A CalDAV
    /// account is a standalone account entry addressed by its
    /// `collection_url` and its own keyring credential; it does not use the
    /// IMAP/JMAP/SMTP mail endpoints.
    CalDav,
}

impl AccountProtocol {
    /// Whether this protocol speaks WebDAV against a collection URL (as
    /// opposed to a mail protocol addressed by host/port). Such accounts
    /// require [`AccountConfig::collection_url`] and do not require the
    /// IMAP/SMTP mail endpoint fields.
    pub fn is_dav(self) -> bool {
        matches!(self, AccountProtocol::CalDav)
    }
}

/// Security and transport encryption mode for mail protocol streams (IMAP & SMTP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    /// Implicit TLS / SSL connection established immediately upon TCP socket connect (e.g. IMAP 993, SMTPS 465).
    ImplicitTls,
    /// Explicit STARTTLS upgrading plain TCP connection to TLS after initial handshake (e.g. IMAP 143, SMTP 587).
    StartTls,
    /// Plain unencrypted TCP connection without TLS (e.g. Local dev / testing ports 143, 25, 2525).
    Plain,
}

/// Errors returned when validating an [`AccountConfig`].
#[derive(Error, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A mandatory configuration field was left empty or blank.
    #[error("field '{field}' cannot be empty")]
    EmptyField {
        /// Name of the missing or blank field.
        field: &'static str,
    },
    /// The specified email address does not follow RFC 5322 format.
    #[error("invalid email address format")]
    InvalidEmailFormat,
    /// The server port number is invalid (must be 1..=65535).
    #[error("invalid server port number")]
    InvalidPort,
    /// The synchronization interval is too short (minimum 10 seconds).
    #[error("sync interval must be at least 10 seconds")]
    SyncIntervalTooShort,
    /// A DAV-protocol account's collection URL is not a valid http(s) URL.
    #[error("collection_url must be a valid http(s) URL")]
    InvalidCollectionUrl,
    /// Cleartext `TlsMode::Plain` was requested against a non-loopback host,
    /// which would send credentials over the network unencrypted.
    #[error(
        "{endpoint} host '{host}' is not loopback; refusing cleartext Plain TLS mode to a remote host"
    )]
    CleartextToRemoteHost {
        /// Which endpoint the rejected setting applies to (e.g. "imap", "smtp").
        endpoint: &'static str,
        /// The offending non-loopback host.
        host: String,
    },
}

/// Complete configuration schema for a mail & calendar account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Unique identifier for the account configuration.
    pub id: String,
    /// User-friendly display name (e.g. "Work Mail").
    pub name: String,
    /// Primary email address associated with the account.
    pub email_address: String,
    /// Protocol engine type.
    pub protocol: AccountProtocol,
    /// Server hostname or IP address (IMAP/JMAP).
    pub server_host: String,
    /// Server port number (IMAP/JMAP).
    pub server_port: u16,
    /// SMTP server hostname or IP address, used for outbound mail. Distinct
    /// from `server_host` because a mail provider's inbound (IMAP/JMAP) and
    /// outbound (SMTP) endpoints commonly differ.
    pub smtp_host: String,
    /// SMTP server port number, used for outbound mail.
    pub smtp_port: u16,
    /// Whether TLS connection encryption is enabled.
    pub use_tls: bool,
    /// IMAP connection security transport mode.
    pub imap_tls_mode: TlsMode,
    /// SMTP connection security transport mode.
    pub smtp_tls_mode: TlsMode,
    /// Keyring entry key used to retrieve OS vault credentials.
    pub keyring_secret_key: String,
    /// Background sync polling interval in seconds (minimum 10s).
    pub sync_interval_secs: u64,
    /// Fully-qualified collection URL for DAV-protocol accounts (CalDAV
    /// today, CardDAV later). Set for accounts whose protocol speaks WebDAV;
    /// empty for mail accounts. This is a full URL (scheme + host + path),
    /// not a bare host, because a DAV client dispatches requests directly at
    /// the collection URL.
    pub collection_url: String,
}

/// Whether `host` refers to the local loopback interface, either as a
/// literal IP address (`127.0.0.1`, `::1`, ...) or the conventional
/// `localhost` name. Used to allow cleartext `TlsMode::Plain` only against
/// local test servers, never against a remote host.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

impl AccountConfig {
    /// Minimum allowed background sync interval (10 seconds).
    pub const MIN_SYNC_INTERVAL_SECS: u64 = 10;

    /// Validate the account configuration fields.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.id.trim().is_empty() {
            return Err(ConfigError::EmptyField { field: "id" });
        }
        if self.name.trim().is_empty() {
            return Err(ConfigError::EmptyField { field: "name" });
        }
        if self.email_address.trim().is_empty() {
            return Err(ConfigError::EmptyField {
                field: "email_address",
            });
        }
        if !self.email_address.contains('@')
            || !self
                .email_address
                .split_once('@')
                .is_some_and(|(_, domain)| domain.contains('.'))
        {
            return Err(ConfigError::InvalidEmailFormat);
        }
        if self.protocol.is_dav() {
            // A DAV-protocol account is addressed by its collection URL, not
            // by mail host/port endpoints, so it validates the URL instead of
            // the IMAP/SMTP fields (which are left empty for such accounts).
            let url = self.collection_url.trim();
            if url.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "collection_url",
                });
            }
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(ConfigError::InvalidCollectionUrl);
            }
        } else {
            if self.server_host.trim().is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "server_host",
                });
            }
            if self.server_port == 0 {
                return Err(ConfigError::InvalidPort);
            }
            if self.smtp_host.trim().is_empty() {
                return Err(ConfigError::EmptyField { field: "smtp_host" });
            }
            if self.smtp_port == 0 {
                return Err(ConfigError::InvalidPort);
            }
            if self.imap_tls_mode == TlsMode::Plain && !is_loopback_host(&self.server_host) {
                return Err(ConfigError::CleartextToRemoteHost {
                    endpoint: "imap",
                    host: self.server_host.clone(),
                });
            }
            if self.smtp_tls_mode == TlsMode::Plain && !is_loopback_host(&self.smtp_host) {
                return Err(ConfigError::CleartextToRemoteHost {
                    endpoint: "smtp",
                    host: self.smtp_host.clone(),
                });
            }
        }
        if self.keyring_secret_key.trim().is_empty() {
            return Err(ConfigError::EmptyField {
                field: "keyring_secret_key",
            });
        }
        if self.sync_interval_secs < Self::MIN_SYNC_INTERVAL_SECS {
            return Err(ConfigError::SyncIntervalTooShort);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_account() -> AccountConfig {
        AccountConfig {
            id: "acct-123".to_string(),
            name: "Personal Mail".to_string(),
            email_address: "user@nuncio.mx".to_string(),
            protocol: AccountProtocol::Jmap,
            server_host: "jmap.nuncio.mx".to_string(),
            server_port: 443,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
            use_tls: true,
            imap_tls_mode: TlsMode::ImplicitTls,
            smtp_tls_mode: TlsMode::ImplicitTls,
            keyring_secret_key: "nuncio/acct-123".to_string(),
            sync_interval_secs: 60,
            collection_url: String::new(),
        }
    }

    fn valid_caldav_account() -> AccountConfig {
        AccountConfig {
            id: "acct-cal-1".to_string(),
            name: "Work Calendar".to_string(),
            email_address: "user@nuncio.mx".to_string(),
            protocol: AccountProtocol::CalDav,
            server_host: String::new(),
            server_port: 0,
            smtp_host: String::new(),
            smtp_port: 0,
            use_tls: true,
            imap_tls_mode: TlsMode::ImplicitTls,
            smtp_tls_mode: TlsMode::ImplicitTls,
            keyring_secret_key: "nuncio/acct-cal-1".to_string(),
            sync_interval_secs: 300,
            collection_url: "https://caldav.example.com/calendars/user/work/".to_string(),
        }
    }

    #[test]
    fn valid_caldav_account_passes_without_mail_endpoints() {
        let config = valid_caldav_account();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn caldav_account_requires_collection_url() {
        let mut config = valid_caldav_account();
        config.collection_url = "  ".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField {
                field: "collection_url"
            }
        );
    }

    #[test]
    fn caldav_account_rejects_non_http_collection_url() {
        let mut config = valid_caldav_account();
        config.collection_url = "ftp://caldav.example.com/work/".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::InvalidCollectionUrl
        );
    }

    #[test]
    fn account_protocol_is_dav_only_for_dav_protocols() {
        assert!(AccountProtocol::CalDav.is_dav());
        assert!(!AccountProtocol::Jmap.is_dav());
        assert!(!AccountProtocol::ImapSmtp.is_dav());
    }

    #[test]
    fn valid_account_config_passes_validation() {
        let config = valid_account();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn empty_id_fails_validation() {
        let mut config = valid_account();
        config.id = "   ".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField { field: "id" }
        );
    }

    #[test]
    fn empty_name_fails_validation() {
        let mut config = valid_account();
        config.name = "".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField { field: "name" }
        );
    }

    #[test]
    fn empty_email_fails_validation() {
        let mut config = valid_account();
        config.email_address = "".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField {
                field: "email_address"
            }
        );
    }

    #[test]
    fn invalid_email_format_fails_validation() {
        let mut config = valid_account();
        config.email_address = "invalidemail".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::InvalidEmailFormat
        );

        config.email_address = "user@domainnodot".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::InvalidEmailFormat
        );
    }

    #[test]
    fn empty_server_host_fails_validation() {
        let mut config = valid_account();
        config.server_host = " ".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField {
                field: "server_host"
            }
        );
    }

    #[test]
    fn zero_port_fails_validation() {
        let mut config = valid_account();
        config.server_port = 0;
        assert_eq!(config.validate().unwrap_err(), ConfigError::InvalidPort);
    }

    #[test]
    fn empty_smtp_host_fails_validation() {
        let mut config = valid_account();
        config.smtp_host = " ".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField { field: "smtp_host" }
        );
    }

    #[test]
    fn zero_smtp_port_fails_validation() {
        let mut config = valid_account();
        config.smtp_port = 0;
        assert_eq!(config.validate().unwrap_err(), ConfigError::InvalidPort);
    }

    #[test]
    fn empty_keyring_key_fails_validation() {
        let mut config = valid_account();
        config.keyring_secret_key = "".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField {
                field: "keyring_secret_key"
            }
        );
    }

    #[test]
    fn sync_interval_too_short_fails_validation() {
        let mut config = valid_account();
        config.sync_interval_secs = 5;
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::SyncIntervalTooShort
        );
    }

    #[test]
    fn plain_imap_to_remote_host_fails_validation() {
        let mut config = valid_account();
        config.imap_tls_mode = TlsMode::Plain;
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::CleartextToRemoteHost {
                endpoint: "imap",
                host: "jmap.nuncio.mx".to_string(),
            }
        );
    }

    #[test]
    fn plain_smtp_to_remote_host_fails_validation() {
        let mut config = valid_account();
        config.smtp_tls_mode = TlsMode::Plain;
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::CleartextToRemoteHost {
                endpoint: "smtp",
                host: "smtp.nuncio.mx".to_string(),
            }
        );
    }

    #[test]
    fn plain_imap_and_smtp_to_loopback_hosts_pass_validation() {
        for host in ["127.0.0.1", "localhost", "::1", "LOCALHOST"] {
            let mut config = valid_account();
            config.server_host = host.to_string();
            config.smtp_host = host.to_string();
            config.imap_tls_mode = TlsMode::Plain;
            config.smtp_tls_mode = TlsMode::Plain;
            assert!(config.validate().is_ok(), "host {host} should be allowed");
        }
    }

    #[test]
    fn implicit_tls_and_starttls_to_remote_host_pass_validation() {
        let mut config = valid_account();
        config.imap_tls_mode = TlsMode::ImplicitTls;
        config.smtp_tls_mode = TlsMode::StartTls;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn account_protocol_serde_roundtrip() {
        let jmap_json = serde_json::to_string(&AccountProtocol::Jmap).unwrap();
        assert_eq!(jmap_json, "\"jmap\"");
        let parsed: AccountProtocol = serde_json::from_str(&jmap_json).unwrap();
        assert_eq!(parsed, AccountProtocol::Jmap);

        let imap_json = serde_json::to_string(&AccountProtocol::ImapSmtp).unwrap();
        assert_eq!(imap_json, "\"imap-smtp\"");
        let parsed_imap: AccountProtocol = serde_json::from_str(&imap_json).unwrap();
        assert_eq!(parsed_imap, AccountProtocol::ImapSmtp);

        let caldav_json = serde_json::to_string(&AccountProtocol::CalDav).unwrap();
        assert_eq!(caldav_json, "\"cal-dav\"");
        let parsed_caldav: AccountProtocol = serde_json::from_str(&caldav_json).unwrap();
        assert_eq!(parsed_caldav, AccountProtocol::CalDav);
    }

    #[test]
    fn tls_mode_serde_roundtrip() {
        assert_eq!(
            serde_json::to_string(&TlsMode::ImplicitTls).unwrap(),
            "\"implicit_tls\""
        );
        assert_eq!(
            serde_json::to_string(&TlsMode::StartTls).unwrap(),
            "\"start_tls\""
        );
        assert_eq!(serde_json::to_string(&TlsMode::Plain).unwrap(), "\"plain\"");
    }

    #[test]
    fn full_account_config_serde_roundtrip() {
        let config = valid_account();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AccountConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, config);
    }

    #[test]
    fn error_display_formatting() {
        assert_eq!(
            ConfigError::EmptyField { field: "name" }.to_string(),
            "field 'name' cannot be empty"
        );
        assert_eq!(
            ConfigError::InvalidEmailFormat.to_string(),
            "invalid email address format"
        );
        assert_eq!(
            ConfigError::InvalidPort.to_string(),
            "invalid server port number"
        );
        assert_eq!(
            ConfigError::SyncIntervalTooShort.to_string(),
            "sync interval must be at least 10 seconds"
        );
        assert_eq!(
            ConfigError::CleartextToRemoteHost {
                endpoint: "imap",
                host: "mail.example.com".to_string(),
            }
            .to_string(),
            "imap host 'mail.example.com' is not loopback; refusing cleartext Plain TLS mode to a remote host"
        );
    }
}
