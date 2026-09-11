use crate::{
    args::{Args, Command, SystemCommand},
    output::AppError,
    rpc_error,
};
use nuncio_proto::v2::{system_client::SystemClient, WatchChangesRequest};
use std::io::Write;

pub async fn run(args: Args) -> Result<(), AppError> {
    let (after, limit) = match &args.command {
        Command::System {
            command: SystemCommand::Watch { after, limit },
        } => (*after, *limit),
        _ => return Err(AppError::invalid()),
    };
    let (channel, token) = crate::connect(&args).await?;
    let mut stream = SystemClient::with_interceptor(channel, token)
        .watch_changes(WatchChangesRequest {
            after_revision: after,
        })
        .await
        .map_err(rpc_error)?
        .into_inner();
    let mut previous = after;
    let mut emitted = 0_u64;
    loop {
        let change = tokio::select! {
            result=stream.message()=>result.map_err(rpc_error)?.ok_or_else(AppError::unavailable)?,
            result=tokio::signal::ctrl_c()=>{result.map_err(|_|AppError::unavailable())?;return Ok(());}
        };
        if previous.checked_add(1) != Some(change.revision) {
            return Err(AppError {
                code: "resnapshot_required",
                message: "Change revisions are discontinuous; take a new snapshot",
                sync_run: None,
                operation: None,
                recovery: None,
                exit: 5,
            });
        }
        previous = change.revision;
        let value = serde_json::json!({"schema_version":1,"result":change});
        let line = crate::output::render(&value, true).map_err(|_| output_error())?;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{line}")
            .and_then(|()| stdout.flush())
            .map_err(|_| output_error())?;
        emitted += 1;
        if limit.is_some_and(|n| emitted >= u64::from(n)) {
            return Ok(());
        }
    }
}
fn output_error() -> AppError {
    AppError {
        code: "output_failed",
        message: "Could not write change output",
        sync_run: None,
        operation: None,
        recovery: None,
        exit: 1,
    }
}
