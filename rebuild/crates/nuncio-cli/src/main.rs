mod accounts;
mod args;
mod backup;
mod calendar;
mod changes;
mod credentials;
mod draft_upload;
mod drafts;
mod help_text;
mod human;
mod mail;
mod operations;
mod output;
mod output_file;
mod repair;
mod usage;

use args::{Args, Command, SystemCommand};
use clap::Parser;
use nuncio_proto::{
    client::{ClientError, TokenInjector},
    v2::{system_client::SystemClient, GetStatusRequest, ShutdownRequest},
};
use output::AppError;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            return if error.print().is_ok() {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::from(1)
            };
        }
        Err(error)
            if error.kind() == clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                && !std::env::args_os().any(|value| value == "--json") =>
        {
            return if error.print().is_ok() {
                std::process::ExitCode::from(2)
            } else {
                std::process::ExitCode::from(1)
            };
        }
        Err(error) => {
            return if std::env::args_os().any(|value| value == "--json") {
                output::emit(Err(AppError::invalid()), true)
            } else {
                usage::emit(&error)
            };
        }
    };
    let json = args.json;
    if matches!(
        &args.command,
        Command::System {
            command: SystemCommand::Watch { .. }
        }
    ) {
        return match changes::run(args).await {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => output::emit(Err(e), true),
        };
    }
    let guided = matches!(
        &args.command,
        Command::Account {
            command: args::AccountCommand::Add { .. }
        }
    );
    if guided {
        if let Err(error) = accounts::wizard::preflight(json) {
            return output::emit(Err(error), json);
        }
        return output::emit_setup(run(args).await);
    }
    output::emit(run(args).await, json)
}

async fn connect(args: &Args) -> Result<(tonic::transport::Channel, TokenInjector), AppError> {
    if std::env::vars_os().any(|(name, _)| name.to_string_lossy().starts_with("NUNCIO_TEST_")) {
        return Err(AppError::invalid());
    }
    let channel = nuncio_proto::client::connect_with_timeout(
        &args.endpoint,
        std::time::Duration::from_secs(if matches!(args.command, Command::Backup { .. }) {
            3600
        } else {
            30
        }),
    )
    .await
    .map_err(|error| match error {
        ClientError::InvalidEndpoint => AppError::invalid(),
        ClientError::Authorization => AppError::auth(),
        ClientError::Unavailable => AppError::unavailable(),
    })?;
    let token = credentials::load(args)?;
    let injector = TokenInjector::new(token).map_err(|_| AppError::auth())?;
    Ok((channel, injector))
}
async fn run(args: Args) -> Result<serde_json::Value, AppError> {
    let (channel, injector) = connect(&args).await?;
    if let Command::Repair(command) = args.command {
        return repair::run(command, channel, injector).await;
    }
    if let Command::Backup { command } = args.command {
        return backup::run(command, channel, injector).await;
    }
    if let Command::Operations { command } = args.command {
        return operations::run(command, channel, injector).await;
    }
    if let Command::Calendar { command } = args.command {
        return calendar::run(command, channel, injector).await;
    }
    if let Command::Account { command } = args.command {
        return accounts::run(command, channel, injector).await;
    }
    if let Command::Mail { command } = args.command {
        return mail::run(command, channel, injector).await;
    }
    let mut client = SystemClient::with_interceptor(channel, injector);
    match args.command {
        Command::Repair(_) => Err(AppError::invalid()),
        Command::Backup { .. } => Err(AppError::invalid()),
        Command::Operations { .. } => Err(AppError::invalid()),
        Command::System {
            command: SystemCommand::Watch { .. },
        } => Err(AppError::invalid()),
        Command::Calendar { .. } => Err(AppError::invalid()),
        Command::Account { .. } => Err(AppError::invalid()),
        Command::Mail { .. } => Err(AppError::invalid()),
        Command::Sync {
            account,
            full,
            wait,
        } => mail::sync(&mut client, account, full, None, wait).await,
        Command::System {
            command: SystemCommand::SyncStatus { account, run },
        } => {
            let run = client
                .get_sync_run(nuncio_proto::v2::SyncRunRequest {
                    account_id: account,
                    run_id: run,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            mail::json(run)
        }
        Command::System {
            command: SystemCommand::CancelSync { account, run },
        } => {
            let run = client
                .cancel_sync(nuncio_proto::v2::SyncRunRequest {
                    account_id: account,
                    run_id: run,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            mail::json(run)
        }
        Command::System {
            command: SystemCommand::Status,
        } => {
            let response = client
                .get_status(GetStatusRequest {})
                .await
                .map_err(rpc_error)?
                .into_inner();
            let storage = response.storage.ok_or_else(AppError::unavailable)?;
            Ok(
                serde_json::json!({"version":response.version, "api_version":response.api_version, "profile_id":response.profile_id,
                "storage":{"schema_version":storage.schema_version,"account_count":storage.account_count,"revision":storage.revision},
                "protected_paths":response.protected_paths,"background_sync":response.background_sync,"scheduler_error":response.scheduler_error,"operation_worker_error":response.operation_worker_error,"sync":response.sync,"resources":response.resources}),
            )
        }
        Command::System {
            command: SystemCommand::Shutdown,
        } => {
            client
                .shutdown(ShutdownRequest {})
                .await
                .map_err(rpc_error)?;
            Ok(serde_json::json!({"shutdown_requested":true}))
        }
    }
}

fn rpc_error(status: tonic::Status) -> AppError {
    let guidance = rpc_guidance(&status);
    let mut error = match status.code() {
        tonic::Code::OutOfRange => AppError {
            code: "resnapshot_required",
            message:
                "Change history is unavailable; take a new snapshot and resume from its revision",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 5,
        },
        tonic::Code::Unauthenticated | tonic::Code::PermissionDenied => AppError::auth(),
        tonic::Code::InvalidArgument => AppError::invalid(),
        tonic::Code::Unavailable | tonic::Code::DeadlineExceeded => AppError::unavailable(),
        tonic::Code::Unknown | tonic::Code::Cancelled => AppError {
            code: "request_outcome_unknown",
            message: "Request outcome is unknown; inspect state or retry the identical request",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 4,
        },
        tonic::Code::NotFound => AppError {
            code: "not_found",
            message: "Resource not found for this account; copy its local ID from the matching list command and check --account",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 4,
        },
        tonic::Code::FailedPrecondition | tonic::Code::Aborted => AppError {
            code: "conflict",
            message: "This action conflicts with current state; inspect the account/resource and use its latest version or ETag before retrying",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 5,
        },
        _ => AppError {
            code: "operation_failed",
            message: "The engine could not complete this action; inspect the daemon log and any returned operation or sync run",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 1,
        },
    };
    if let Some(message) = guidance {
        error.message = message;
    }
    error
}

fn rpc_guidance(status: &tonic::Status) -> Option<&'static str> {
    use tonic::Code;
    // Recognize only fixed API messages; never print arbitrary server details.
    Some(match (status.code(), status.message()) {
        (Code::Unauthenticated, "Account authorization is required") => "Account sign-in is required. Use account add for a new account, or account reauth-google / account reauth-imap for a saved account; see their --help.",
        (Code::Unauthenticated, "Required Google permissions were not granted") => "Required Google permissions were not granted. Repeat Google sign-in and grant the requested Gmail and Calendar access; see docs/GOOGLE-SETUP.md.",
        (Code::FailedPrecondition, "Mail server TLS verification failed") => "Mail server TLS verification failed. Check the server hostname, certificate and trusted CA in the account settings; use account add or account edit-imap.",
        (Code::FailedPrecondition, "Mail server does not support a required protocol capability") => "The mail server lacks a required protocol capability; compare its settings with docs/COMPATIBILITY.md.",
        (Code::FailedPrecondition, "Provider account identity does not match the saved account") => "Sign-in selected a different account. Reauthenticate using the saved identity, or use account add for a separate account.",
        (Code::FailedPrecondition, "Account lifecycle does not permit this action; restore archived accounts and resolve remote-effect uncertainty before deletion") => "Account state prevents this action. Inspect account show; restore an archived account before reconnecting and resolve uncertain operations before permanent deletion.",
        (Code::Aborted, "Account configuration changed; read the latest version") => "The account configuration changed. Read account show and repeat the edit using its current --version.",
        (Code::FailedPrecondition, "The local resource changed; read its latest version before editing") => "The resource changed. Read its show/get result and repeat the edit using the current version or ETag.",
        (Code::FailedPrecondition, "The query snapshot changed; restart pagination") => "Cached results changed during pagination. Start the list/search again without --page-token, then use its new page tokens.",
        (Code::FailedPrecondition, "Different work is already active for this account and scope") => "Other work is active for this account and scope. Inspect its sync run or operation and wait for it to finish before retrying.",
        (Code::Unavailable, "Provider service is unavailable") => "The provider is unavailable. Check account status and daemon logs; inspect any queued operation before retrying a write.",
        (Code::Unavailable, "Provider service requested a later attempt") => "The provider requested a delay. Wait before checking again; queued work retains its retry schedule.",
        (Code::ResourceExhausted, "Too many pending authorization sessions or accounts") => "Too many accounts or pending sign-in sessions. Inspect account list and cancel unused sessions with account auth-cancel.",
        (Code::ResourceExhausted, "Query result is too large; request a smaller page") => "The result exceeds the response limit. Retry with a smaller --page-size.",
        (Code::InvalidArgument, "Invalid account configuration") => "The account configuration is invalid. Check account setup help, or use account add for guided input; see docs/ACCOUNT-SETUP.md.",
        (Code::Internal, "Secure credential storage is unavailable") => "The secure credential store is unavailable. Unlock the OS credential store and inspect the daemon log before retrying account setup.",
        _ => return None,
    })
}

#[cfg(test)]
mod rpc_error_tests {
    #[test]
    fn provider_errors_explain_the_right_recovery_without_echoing_server_text() {
        use tonic::{Code, Status};
        for (code, message, guidance) in [
            (
                Code::Unauthenticated,
                "Account authorization is required",
                "reauth",
            ),
            (
                Code::Unauthenticated,
                "Required Google permissions were not granted",
                "Google permissions",
            ),
            (
                Code::FailedPrecondition,
                "Mail server TLS verification failed",
                "certificate",
            ),
            (
                Code::Unavailable,
                "Provider service is unavailable",
                "provider",
            ),
            (
                Code::FailedPrecondition,
                "The query snapshot changed; restart pagination",
                "--page-token",
            ),
        ] {
            let error = super::rpc_error(Status::new(code, message));
            assert!(
                error.message.contains(guidance),
                "wrong recovery for {message}: {}",
                error.message
            );
            assert!(!error.message.contains("start 'nunciod"));
        }
        let local = super::rpc_error(Status::unauthenticated("Profile authorization required"));
        assert!(local.message.contains("--profile"));
        for code in [
            Code::Unauthenticated,
            Code::FailedPrecondition,
            Code::Unavailable,
            Code::Internal,
        ] {
            let error = super::rpc_error(Status::new(code, "private-server-token\u{1b}[2J"));
            assert!(!error.message.contains("private-server-token"));
            assert!(!error.message.contains('\u{1b}'));
        }
    }
}
