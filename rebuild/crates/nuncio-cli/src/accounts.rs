mod imap;
use crate::{args::AccountCommand, output::AppError, rpc_error};
use nuncio_proto::{
    client::TokenInjector,
    v2::{self, accounts_client::AccountsClient},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io::Read, path::Path};
use tonic::transport::Channel;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
struct Registration {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}
#[derive(Deserialize)]
struct Downloaded {
    installed: Registration,
}

fn registration(path: &Path) -> Result<Registration, AppError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| AppError::invalid())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 65536 {
        return Err(AppError::invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(AppError {
                code: "insecure_file",
                message:
                    "OAuth client configuration must be readable only by its owner (chmod 600)",
                sync_run: None,
                operation: None,
                recovery: None,
                exit: 2,
            });
        }
    }
    let mut bytes = Zeroizing::new(Vec::new());
    std::fs::File::open(path)
        .map_err(|_| AppError::invalid())?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::invalid())?;
    if bytes.len() > 65536 {
        return Err(AppError::invalid());
    }
    let config: Downloaded = serde_json::from_slice(&bytes).map_err(|_| AppError::invalid())?;
    Ok(config.installed)
}

pub async fn run(
    command: AccountCommand,
    channel: Channel,
    injector: TokenInjector,
) -> Result<Value, AppError> {
    let mut client = AccountsClient::with_interceptor(channel, injector);
    match command {
        AccountCommand::ConnectImap {
            config,
            credentials_stdin: _,
            account: id,
        } => {
            let config = imap::configuration(&config)?;
            let credentials = tokio::task::spawn_blocking(imap::credentials)
                .await
                .map_err(|_| AppError::invalid())??;
            let result = client
                .connect_imap(v2::ConnectImapRequest {
                    config: Some(config),
                    credentials: Some(credentials),
                    account_id: id,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            Ok(
                json!({"account":account(result.account.ok_or_else(AppError::invalid)?),"capabilities":result.capabilities,"credential_cleanup_pending":result.credential_cleanup_pending}),
            )
        }
        AccountCommand::ImapConfig { account: id } => {
            let result = client
                .get_imap_config(v2::AccountRequest {
                    account_id: id.clone(),
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            Ok(
                json!({"account_id":id,"config":imap::public_config(result.config.ok_or_else(AppError::invalid)?)?,"capabilities":result.capabilities}),
            )
        }
        AccountCommand::ConnectGoogle {
            client_config,
            login_hint,
            account,
            no_browser,
        } => {
            let mut registration = registration(&client_config)?;
            let result = client
                .begin_google_auth(v2::BeginGoogleAuthRequest {
                    client_id: std::mem::take(&mut registration.client_id),
                    client_secret: registration.client_secret.take(),
                    login_hint,
                    account_id: account,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            let opened = if no_browser {
                false
            } else {
                open_browser(&result.browser_url).await
            };
            let mut value = session(result);
            value["browser_opened"] = json!(opened);
            Ok(value)
        }
        AccountCommand::AuthStatus { session: id } => Ok(session(
            client
                .get_auth_status(v2::GetAuthStatusRequest { session_id: id })
                .await
                .map_err(rpc_error)?
                .into_inner(),
        )),
        AccountCommand::List => {
            let result = client
                .list_accounts(v2::ListAccountsRequest {})
                .await
                .map_err(rpc_error)?
                .into_inner();
            Ok(json!({"accounts":result.accounts.into_iter().map(account).collect::<Vec<_>>()}))
        }
        AccountCommand::Disconnect { account: id } => {
            client
                .disconnect_account(v2::AccountRequest {
                    account_id: id.clone(),
                })
                .await
                .map_err(rpc_error)?;
            Ok(json!({"account_id":id,"state":"disconnected"}))
        }
        AccountCommand::Check { account: id } => Ok(account(
            client
                .check_account(v2::AccountRequest { account_id: id })
                .await
                .map_err(rpc_error)?
                .into_inner(),
        )),
    }
}
fn session(value: v2::AuthSession) -> Value {
    json!({"session_id":value.session_id,"browser_url":value.browser_url,"expires_at_ms":value.expires_at_ms,
        "state":value.state,"account_id":value.account_id,"error_code":value.error_code,"warning_code":value.warning_code})
}
fn account(value: v2::Account) -> Value {
    json!({"id":value.id,"provider":value.provider,"address":value.address,"state":value.state})
}
async fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let command = "/usr/bin/open";
    #[cfg(not(target_os = "macos"))]
    let command = "xdg-open";
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new(command)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    {
        Ok(Ok(status)) if status.success() => true,
        _ => {
            eprintln!("Browser could not be opened; use the returned browser_url.");
            false
        }
    }
}
