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

/// IMAP/SMTP transport parameters: inbound mail is fetched over IMAP4rev1 and
/// outbound mail is submitted over SMTP. Inbound and outbound carry
/// independent host/port/TLS settings because a provider's IMAP and SMTP
/// endpoints commonly differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapSmtpTransport {
    /// IMAP server hostname or IP address (inbound mail).
    pub imap_host: String,
    /// IMAP server port number.
    pub imap_port: u16,
    /// IMAP connection security transport mode.
    pub imap_tls_mode: TlsMode,
    /// SMTP server hostname or IP address (outbound mail).
    pub smtp_host: String,
    /// SMTP server port number.
    pub smtp_port: u16,
    /// SMTP connection security transport mode.
    pub smtp_tls_mode: TlsMode,
}

/// JMAP transport parameters (RFC 8620 / RFC 8621). A JMAP account addresses
/// its provider by a session endpoint host; the daemon derives the well-known
/// session URL from it. JMAP always runs over HTTPS, so there is no separate
/// cleartext TLS mode to configure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JmapTransport {
    /// JMAP session endpoint host (the daemon derives the session URL from it).
    pub endpoint_host: String,
}

/// DAV transport parameters (CalDAV RFC 4791 today, CardDAV later). A DAV
/// account is addressed by a fully-qualified collection URL rather than a mail
/// host/port, because a DAV client dispatches requests directly at that URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DavTransport {
    /// Fully-qualified collection URL (scheme + host + path).
    pub collection_url: String,
}

/// The single transport an account speaks. Modeling it as an enum makes a
/// nonsensical mix of transport settings unrepresentable: an account carries
/// exactly one variant, so the IMAP/SMTP, JMAP, and DAV parameters can never
/// be populated simultaneously.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// IMAP inbound + SMTP outbound mail.
    ImapSmtp(ImapSmtpTransport),
    /// JMAP mail.
    Jmap(JmapTransport),
    /// WebDAV collection (CalDAV today, CardDAV later).
    Dav(DavTransport),
}

impl Transport {
    /// The [`AccountProtocol`] discriminant this transport corresponds to.
    pub fn protocol(&self) -> AccountProtocol {
        match self {
            Transport::ImapSmtp(_) => AccountProtocol::ImapSmtp,
            Transport::Jmap(_) => AccountProtocol::Jmap,
            Transport::Dav(_) => AccountProtocol::CalDav,
        }
    }
}

/// Complete configuration schema for a mail & calendar account. The account's
/// common identity fields live here; its transport-specific parameters live in
/// exactly one [`Transport`] variant (see that type's doc comment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Unique identifier for the account configuration.
    pub id: String,
    /// User-friendly display name (e.g. "Work Mail").
    pub name: String,
    /// Primary email address associated with the account.
    pub email_address: String,
    /// Keyring entry key used to retrieve OS vault credentials.
    pub keyring_secret_key: String,
    /// Background sync polling interval in seconds (minimum 10s).
    pub sync_interval_secs: u64,
    /// Whether this daemon executes filter rules for this account.
    ///
    /// **Defaults to `false`**, which is the conservative answer to a problem
    /// the engine cannot solve on its own. Every daemon syncing an account
    /// decides independently that a message is new, so every daemon fires the
    /// same rules against it. `MOVE`/`COPY`/`FLAG` survive that -- ten moves
    /// converge on one outcome -- but `FORWARD` and `CALL WEBHOOK` land on a
    /// third party with no shared state to compare against, so N daemons send
    /// N forwards and N webhook calls.
    ///
    /// Enabling this on exactly one daemon per account makes filter execution
    /// single-owner. It is a deliberate, documented limitation rather than a
    /// silent duplication bug; a server-side claim is the eventual fix where
    /// the server supports one.
    pub filters_enabled: bool,
    /// The account's single transport.
    pub transport: Transport,
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

/// Extract the host portion of an `http://`/`https://` URL, without pulling
/// in a full URL-parsing dependency for this one field. Handles a bracketed
/// IPv6 literal (`[::1]`) as well as a plain hostname/IPv4 literal, and
/// stops at the first `:` (port), `/` (path), `?`, or `#` that follows.
fn url_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    if let Some(bracketed) = rest.strip_prefix('[') {
        let end = bracketed.find(']')?;
        return Some(&bracketed[..end]);
    }
    let end = rest.find(['/', ':', '?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

impl AccountConfig {
    /// Minimum allowed background sync interval (10 seconds).
    pub const MIN_SYNC_INTERVAL_SECS: u64 = 10;

    /// The [`AccountProtocol`] discriminant of this account's transport.
    pub fn protocol(&self) -> AccountProtocol {
        self.transport.protocol()
    }

    /// Whether this account speaks WebDAV against a collection URL.
    pub fn is_dav(&self) -> bool {
        matches!(self.transport, Transport::Dav(_))
    }

    /// The account's IMAP/SMTP transport parameters, present only for an
    /// IMAP/SMTP account (the only transport with an SMTP outbound endpoint).
    pub fn imap_smtp(&self) -> Option<&ImapSmtpTransport> {
        match &self.transport {
            Transport::ImapSmtp(t) => Some(t),
            _ => None,
        }
    }

    /// The account's DAV collection URL, present only for a DAV account.
    pub fn dav_collection_url(&self) -> Option<&str> {
        match &self.transport {
            Transport::Dav(t) => Some(t.collection_url.as_str()),
            _ => None,
        }
    }

    /// Validate the account configuration fields. Exactly one transport is
    /// always present (guaranteed by the [`Transport`] enum); each variant's
    /// endpoint fields are validated against the rules that apply to it,
    /// including the cleartext-to-remote-host refusal for any plain endpoint.
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
        match &self.transport {
            Transport::ImapSmtp(t) => {
                if t.imap_host.trim().is_empty() {
                    return Err(ConfigError::EmptyField { field: "imap_host" });
                }
                if t.imap_port == 0 {
                    return Err(ConfigError::InvalidPort);
                }
                if t.smtp_host.trim().is_empty() {
                    return Err(ConfigError::EmptyField { field: "smtp_host" });
                }
                if t.smtp_port == 0 {
                    return Err(ConfigError::InvalidPort);
                }
                // Cleartext `Plain` is only tolerated against a loopback host
                // (a local test server); a remote host would send the
                // account's credential over the network unencrypted.
                if t.imap_tls_mode == TlsMode::Plain && !is_loopback_host(&t.imap_host) {
                    return Err(ConfigError::CleartextToRemoteHost {
                        endpoint: "imap",
                        host: t.imap_host.clone(),
                    });
                }
                if t.smtp_tls_mode == TlsMode::Plain && !is_loopback_host(&t.smtp_host) {
                    return Err(ConfigError::CleartextToRemoteHost {
                        endpoint: "smtp",
                        host: t.smtp_host.clone(),
                    });
                }
            }
            Transport::Jmap(t) => {
                if t.endpoint_host.trim().is_empty() {
                    return Err(ConfigError::EmptyField {
                        field: "endpoint_host",
                    });
                }
            }
            Transport::Dav(t) => {
                // A DAV account is addressed by its collection URL, not by mail
                // host/port endpoints.
                let url = t.collection_url.trim();
                if url.is_empty() {
                    return Err(ConfigError::EmptyField {
                        field: "collection_url",
                    });
                }
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(ConfigError::InvalidCollectionUrl);
                }
                // Plain `http://` is only tolerated against a loopback host; any
                // remote host must use `https://` to avoid sending the
                // account's Basic-auth credentials in cleartext.
                if url.starts_with("http://") {
                    let host = url_host(url).unwrap_or_default();
                    if !is_loopback_host(host) {
                        return Err(ConfigError::CleartextToRemoteHost {
                            endpoint: "caldav",
                            host: host.to_string(),
                        });
                    }
                }
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

    fn valid_imap_smtp_transport() -> ImapSmtpTransport {
        ImapSmtpTransport {
            imap_host: "imap.nuncio.mx".to_string(),
            imap_port: 993,
            imap_tls_mode: TlsMode::ImplicitTls,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
            smtp_tls_mode: TlsMode::ImplicitTls,
        }
    }

    /// A valid IMAP/SMTP account. Named `valid_account` (rather than a
    /// transport-specific name) because most common-field validation tests
    /// below use it as their baseline.
    fn valid_account() -> AccountConfig {
        AccountConfig {
            id: "acct-123".to_string(),
            name: "Personal Mail".to_string(),
            email_address: "user@nuncio.mx".to_string(),
            keyring_secret_key: "nuncio/acct-123".to_string(),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: Transport::ImapSmtp(valid_imap_smtp_transport()),
        }
    }

    fn valid_jmap_account() -> AccountConfig {
        AccountConfig {
            id: "acct-jmap-1".to_string(),
            name: "JMAP Mail".to_string(),
            email_address: "user@nuncio.mx".to_string(),
            keyring_secret_key: "nuncio/acct-jmap-1".to_string(),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: Transport::Jmap(JmapTransport {
                endpoint_host: "jmap.nuncio.mx".to_string(),
            }),
        }
    }

    fn valid_caldav_account() -> AccountConfig {
        AccountConfig {
            id: "acct-cal-1".to_string(),
            name: "Work Calendar".to_string(),
            email_address: "user@nuncio.mx".to_string(),
            keyring_secret_key: "nuncio/acct-cal-1".to_string(),
            sync_interval_secs: 300,
            filters_enabled: false,
            transport: Transport::Dav(DavTransport {
                collection_url: "https://caldav.example.com/calendars/user/work/".to_string(),
            }),
        }
    }

    /// Convenience: the account's IMAP/SMTP transport, for tests that mutate a
    /// transport field on a baseline built by [`valid_account`].
    fn imap_smtp_mut(config: &mut AccountConfig) -> &mut ImapSmtpTransport {
        match &mut config.transport {
            Transport::ImapSmtp(t) => t,
            _ => panic!("expected an IMAP/SMTP transport"),
        }
    }

    #[test]
    fn valid_caldav_account_passes_without_mail_endpoints() {
        let config = valid_caldav_account();
        assert!(config.validate().is_ok());
    }

    /// Rebuild `config` with a DAV transport carrying `url`.
    fn with_dav_url(config: &mut AccountConfig, url: &str) {
        config.transport = Transport::Dav(DavTransport {
            collection_url: url.to_string(),
        });
    }

    #[test]
    fn caldav_account_requires_collection_url() {
        let mut config = valid_caldav_account();
        with_dav_url(&mut config, "  ");
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
        with_dav_url(&mut config, "ftp://caldav.example.com/work/");
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::InvalidCollectionUrl
        );
    }

    #[test]
    fn caldav_account_rejects_cleartext_http_to_remote_host() {
        let mut config = valid_caldav_account();
        with_dav_url(&mut config, "http://remote/dav/");
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::CleartextToRemoteHost {
                endpoint: "caldav",
                host: "remote".to_string(),
            }
        );
    }

    #[test]
    fn caldav_account_accepts_https_to_remote_host() {
        let mut config = valid_caldav_account();
        with_dav_url(&mut config, "https://remote/dav/");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn caldav_account_accepts_cleartext_http_to_loopback_host() {
        for url in ["http://127.0.0.1/dav/", "http://localhost/dav/"] {
            let mut config = valid_caldav_account();
            with_dav_url(&mut config, url);
            assert!(config.validate().is_ok(), "url {url} should be allowed");
        }
    }

    #[test]
    fn account_protocol_is_dav_only_for_dav_protocols() {
        assert!(AccountProtocol::CalDav.is_dav());
        assert!(!AccountProtocol::Jmap.is_dav());
        assert!(!AccountProtocol::ImapSmtp.is_dav());
    }

    #[test]
    fn transport_reports_its_protocol_discriminant() {
        assert_eq!(valid_account().protocol(), AccountProtocol::ImapSmtp);
        assert_eq!(valid_jmap_account().protocol(), AccountProtocol::Jmap);
        assert_eq!(valid_caldav_account().protocol(), AccountProtocol::CalDav);
        assert!(valid_caldav_account().is_dav());
        assert!(!valid_account().is_dav());
    }

    #[test]
    fn valid_account_config_passes_validation() {
        assert!(valid_account().validate().is_ok());
        assert!(valid_jmap_account().validate().is_ok());
        assert!(valid_caldav_account().validate().is_ok());
    }

    #[test]
    fn jmap_account_requires_endpoint_host() {
        let mut config = valid_jmap_account();
        config.transport = Transport::Jmap(JmapTransport {
            endpoint_host: "  ".to_string(),
        });
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField {
                field: "endpoint_host"
            }
        );
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
    fn empty_imap_host_fails_validation() {
        let mut config = valid_account();
        imap_smtp_mut(&mut config).imap_host = " ".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField { field: "imap_host" }
        );
    }

    #[test]
    fn zero_port_fails_validation() {
        let mut config = valid_account();
        imap_smtp_mut(&mut config).imap_port = 0;
        assert_eq!(config.validate().unwrap_err(), ConfigError::InvalidPort);
    }

    #[test]
    fn empty_smtp_host_fails_validation() {
        let mut config = valid_account();
        imap_smtp_mut(&mut config).smtp_host = " ".to_string();
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::EmptyField { field: "smtp_host" }
        );
    }

    #[test]
    fn zero_smtp_port_fails_validation() {
        let mut config = valid_account();
        imap_smtp_mut(&mut config).smtp_port = 0;
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
        imap_smtp_mut(&mut config).imap_tls_mode = TlsMode::Plain;
        assert_eq!(
            config.validate().unwrap_err(),
            ConfigError::CleartextToRemoteHost {
                endpoint: "imap",
                host: "imap.nuncio.mx".to_string(),
            }
        );
    }

    #[test]
    fn plain_smtp_to_remote_host_fails_validation() {
        let mut config = valid_account();
        imap_smtp_mut(&mut config).smtp_tls_mode = TlsMode::Plain;
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
            let t = imap_smtp_mut(&mut config);
            t.imap_host = host.to_string();
            t.smtp_host = host.to_string();
            t.imap_tls_mode = TlsMode::Plain;
            t.smtp_tls_mode = TlsMode::Plain;
            assert!(config.validate().is_ok(), "host {host} should be allowed");
        }
    }

    #[test]
    fn implicit_tls_and_starttls_to_remote_host_pass_validation() {
        let mut config = valid_account();
        let t = imap_smtp_mut(&mut config);
        t.imap_tls_mode = TlsMode::ImplicitTls;
        t.smtp_tls_mode = TlsMode::StartTls;
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
