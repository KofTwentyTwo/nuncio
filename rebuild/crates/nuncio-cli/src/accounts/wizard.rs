mod terminal;
use super::{account, google, selected_registration, AccountClient};
use crate::output::AppError;
use nuncio_proto::v2;
use serde_json::{json, Value};
use std::path::PathBuf;
use terminal::Terminal;
use zeroize::Zeroizing;

pub(crate) fn error(code: &'static str, message: &'static str, exit: u8) -> AppError {
    AppError {
        code,
        message,
        exit,
        recovery: None,
        operation: None,
        sync_run: None,
    }
}
pub(crate) fn preflight(json: bool) -> Result<(), AppError> {
    Terminal::check(json)
}

pub(super) async fn run(
    client: &mut AccountClient,
    registration: Option<PathBuf>,
    no_browser: bool,
) -> Result<Value, AppError> {
    let terminal = Terminal::new()?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| AppError::invalid())?;
    let connection = tokio::select! {
        result = setup(&terminal, registration) => result,
        _ = tokio::signal::ctrl_c() => Err(error("setup_cancelled", "Account setup cancelled; no sign-in was submitted.", 130)),
        _ = terminate.recv() => Err(error("setup_cancelled", "Account setup cancelled; no sign-in was submitted.", 130)),
    }?;
    drop(terminal);
    match connection {
        Connection::Google {
            registration,
            address,
        } => {
            // OAuth owns cancellation once a session exists, so a late callback
            // cannot admit an account after a successful cancellation.
            let session = google::connect_registered(
                client,
                registration,
                Some(address),
                None,
                no_browser,
                true,
            )
            .await?;
            let id = session["account_id"]
                .as_str()
                .ok_or_else(AppError::invalid)?;
            let saved = client
                .get_account(v2::AccountRequest {
                    account_id: id.to_owned(),
                })
                .await
                .map_err(crate::rpc_error)?
                .into_inner();
            Ok(json!({"account":account(saved)}))
        }
        Connection::Imap {
            config,
            mut password,
            mut smtp_password,
        } => {
            eprintln!("Checking secure IMAP and SMTP sign-in...");
            let request = v2::ConnectImapRequest {
                config: Some(*config),
                credentials: Some(v2::ImapCredentials {
                    imap_password: std::mem::take(&mut *password),
                    smtp_password: std::mem::take(&mut *smtp_password),
                }),
                account_id: None,
            };
            let result = tokio::select! {
                result = client.connect_imap(request) => result.map_err(|_| error("account_connection_failed", "Could not confirm sign-in. Check your server, login, password and TLS settings. Inspect account list before retrying.", 4)),
                _ = tokio::signal::ctrl_c() => Err(error("setup_interrupted", "Stopped waiting for sign-in. Check account list before retrying; the connection may have completed.", 130)),
                _ = terminate.recv() => Err(error("setup_interrupted", "Stopped waiting for sign-in. Check account list before retrying; the connection may have completed.", 130)),
            }?.into_inner();
            Ok(
                json!({"account":account(result.account.ok_or_else(AppError::invalid)?),"credential_cleanup_pending":result.credential_cleanup_pending}),
            )
        }
    }
}

enum Connection {
    Google {
        registration: super::Registration,
        address: String,
    },
    Imap {
        config: Box<v2::ImapAccountConfig>,
        password: Zeroizing<String>,
        smtp_password: Zeroizing<String>,
    },
}

async fn setup(t: &Terminal, registration: Option<PathBuf>) -> Result<Connection, AppError> {
    t.say("Add an account\n  1. Google (Gmail and Calendar)\n  2. Synology MailPlus / IMAP\nPress Ctrl-C at any time to cancel.")?;
    let provider = loop {
        match t
            .ask("Provider", Some("1"))
            .await?
            .to_ascii_lowercase()
            .as_str()
        {
            "1" | "google" | "gmail" => break true,
            "2" | "imap" | "mailplus" | "synology" => break false,
            _ => t.say("Choose 1 for Google or 2 for MailPlus / IMAP.")?,
        }
    };
    let google_registration = if provider {
        Some(selected_registration(registration)?)
    } else {
        None
    };
    let address = loop {
        let value = t.ask("Email address", None).await?;
        if valid_address(&value) {
            break value;
        }
        t.say("Enter a complete email address, such as name@example.com.")?;
    };
    if let Some(registration) = google_registration {
        t.say("Google will open in your browser. Nuncio requests Gmail and Calendar access, including write permissions. Connecting starts background mail/calendar synchronization.")?;
        confirm(t).await?;
        return Ok(Connection::Google {
            registration,
            address,
        });
    }
    let host = loop {
        let value = t.ask("Mail server", None).await?;
        if valid_host(&value) {
            break value;
        }
        t.say("Enter just the server hostname or IP address, without a URL or port.")?;
    };
    let username = t.ask("Username", Some(&address)).await?;
    let password = t.password("Password (hidden)").await?;
    if password.is_empty() {
        return Err(error(
            "empty_password",
            "A password is required. Account setup stopped.",
            2,
        ));
    }
    let mut smtp_password = Zeroizing::new(password.to_string());
    let mut config = v2::ImapAccountConfig {
        address,
        imap: Some(v2::MailEndpoint {
            host: host.clone(),
            port: 993,
            tls: v2::MailTls::Implicit.into(),
            username: username.clone(),
        }),
        smtp: Some(v2::MailEndpoint {
            host,
            port: 587,
            tls: v2::MailTls::StartTls.into(),
            username,
        }),
        sent_policy: v2::SentPolicy::ClientAppend.into(),
        sent_folder: "Sent".into(),
        archive_folder: None,
        trash_folder: None,
        trusted_ca_pem: None,
    };
    t.say("Defaults: IMAP 993 with TLS; SMTP 587 with STARTTLS; same login for both. Sent-copy policy can be configured before sending. Connecting starts background mailbox synchronization.")?;
    if yes(t, "Change advanced server settings?", false).await? {
        advanced(t, &mut config, &mut smtp_password).await?;
    }
    confirm(t).await?;
    Ok(Connection::Imap {
        config: Box::new(config),
        password,
        smtp_password,
    })
}

async fn yes(t: &Terminal, label: &str, default: bool) -> Result<bool, AppError> {
    loop {
        match t
            .ask(label, Some(if default { "Y/n" } else { "y/N" }))
            .await?
            .to_ascii_lowercase()
            .as_str()
        {
            "y/n" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => t.say("Enter yes or no.")?,
        }
    }
}
async fn confirm(t: &Terminal) -> Result<(), AppError> {
    if yes(t, "Connect and start syncing?", true).await? {
        Ok(())
    } else {
        Err(error(
            "setup_cancelled",
            "Account setup cancelled; no sign-in was submitted.",
            130,
        ))
    }
}
fn valid_address(value: &str) -> bool {
    value.len() <= 320
        && !value.chars().any(char::is_whitespace)
        && value.split_once('@').is_some_and(|(local, host)| {
            !local.is_empty() && !host.contains('@') && valid_host(host)
        })
}
fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && (value.parse::<std::net::IpAddr>().is_ok()
            || value.split('.').all(|s| {
                !s.is_empty()
                    && s.len() <= 63
                    && !s.starts_with('-')
                    && !s.ends_with('-')
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }))
}
async fn port(t: &Terminal, label: &str, current: u32) -> Result<u32, AppError> {
    loop {
        if let Ok(value) = t
            .ask(label, Some(&current.to_string()))
            .await?
            .parse::<u16>()
        {
            if value > 0 {
                return Ok(u32::from(value));
            }
        }
        t.say("Enter a port from 1 to 65535.")?;
    }
}
async fn tls(t: &Terminal, label: &str, current: i32) -> Result<i32, AppError> {
    loop {
        match t
            .ask(
                label,
                Some(if current == v2::MailTls::Implicit as i32 {
                    "1"
                } else {
                    "2"
                }),
            )
            .await?
            .as_str()
        {
            "1" => return Ok(v2::MailTls::Implicit.into()),
            "2" => return Ok(v2::MailTls::StartTls.into()),
            _ => t.say(
                "Choose 1 for TLS or 2 for STARTTLS. Unencrypted connections are not supported.",
            )?,
        }
    }
}
async fn advanced(
    t: &Terminal,
    config: &mut v2::ImapAccountConfig,
    password: &mut Zeroizing<String>,
) -> Result<(), AppError> {
    let imap = config.imap.as_mut().ok_or_else(AppError::invalid)?;
    imap.port = port(t, "IMAP port", imap.port).await?;
    imap.tls = tls(t, "IMAP security: 1 TLS, 2 STARTTLS", imap.tls).await?;
    let smtp = config.smtp.as_mut().ok_or_else(AppError::invalid)?;
    loop {
        let host = t.ask("SMTP server", Some(&smtp.host)).await?;
        if valid_host(&host) {
            smtp.host = host;
            break;
        }
        t.say("Enter a server hostname or IP address.")?;
    }
    smtp.port = port(t, "SMTP port", smtp.port).await?;
    smtp.tls = tls(
        t,
        "SMTP security: 1 TLS, 2 STARTTLS",
        if smtp.port == 465 {
            v2::MailTls::Implicit.into()
        } else {
            smtp.tls
        },
    )
    .await?;
    smtp.username = t.ask("SMTP username", Some(&smtp.username)).await?;
    if yes(t, "Different SMTP password?", false).await? {
        *password = t.password("SMTP password (hidden)").await?;
    }
    let ca = t.ask("Custom CA certificate file (optional)", None).await?;
    if !ca.is_empty() {
        use std::io::Read;
        let mut text = String::new();
        {
            let file = std::fs::File::from(
                rustix::fs::open(
                    ca.as_str(),
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::NONBLOCK
                        | rustix::fs::OFlags::NOFOLLOW,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|_| {
                    error(
                        "certificate_file",
                        "Could not read the custom CA certificate file.",
                        2,
                    )
                })?,
            );
            if !file.metadata().map_err(|_| AppError::invalid())?.is_file() {
                return Err(AppError::invalid());
            }
            file
        }
        .take(256 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|_| AppError::invalid())?;
        if text.len() > 256 * 1024 {
            return Err(AppError::invalid());
        }
        config.trusted_ca_pem = Some(text);
    }
    config.sent_folder = t.ask("Sent folder", Some("Sent")).await?;
    if yes(t, "Does the SMTP server save its own Sent copy?", false).await? {
        config.sent_policy = v2::SentPolicy::Server.into();
    }
    let archive = t.ask("Archive folder (optional)", None).await?;
    config.archive_folder = (!archive.is_empty()).then_some(archive);
    let trash = t.ask("Trash folder (optional)", None).await?;
    config.trash_folder = (!trash.is_empty()).then_some(trash);
    Ok(())
}
