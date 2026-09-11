mod accounts;
mod args;
mod backup;
mod calendar;
mod changes;
mod credentials;
mod draft_upload;
mod drafts;
mod mail;
mod operations;
mod output;
mod output_file;
mod repair;

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
        Err(_) => {
            return output::emit(
                Err(AppError::invalid()),
                std::env::args_os().any(|value| value == "--json"),
            )
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
                "protected_paths":response.protected_paths,"background_sync":response.background_sync,"scheduler_error":response.scheduler_error,"operation_worker_error":response.operation_worker_error,"sync":response.sync}),
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
    match status.code() {
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
            message: "Requested resource was not found",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 4,
        },
        tonic::Code::FailedPrecondition | tonic::Code::Aborted => AppError {
            code: "conflict",
            message: "The requested action conflicts with current state",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 5,
        },
        _ => AppError {
            code: "operation_failed",
            message: "Engine operation failed",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 1,
        },
    }
}
