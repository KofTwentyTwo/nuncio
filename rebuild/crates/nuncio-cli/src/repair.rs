use crate::{
    args::{RepairArgs, RepairScope},
    output::AppError,
};
use nuncio_proto::{
    client::TokenInjector,
    v2::{self, maintenance_client::MaintenanceClient, system_client::SystemClient},
};
use tonic::transport::Channel;
pub async fn run(
    args: RepairArgs,
    channel: Channel,
    injector: TokenInjector,
) -> Result<serde_json::Value, AppError> {
    let scope = match args.scope {
        RepairScope::Mail => v2::ProjectionScope::Mail,
        RepairScope::Calendar => v2::ProjectionScope::Calendar,
    };
    let window = match (args.from, args.to) {
        (Some(from), Some(to)) if scope == v2::ProjectionScope::Calendar => {
            Some(v2::AgendaWindow { from, to })
        }
        (None, None) => None,
        _ => return Err(AppError::invalid()),
    };
    let response = MaintenanceClient::with_interceptor(channel.clone(), injector.clone())
        .repair_projection(v2::RepairProjectionRequest {
            account_id: args.account.clone(),
            scope: scope.into(),
            dry_run: args.dry_run,
            window,
        })
        .await
        .map_err(crate::rpc_error)?
        .into_inner();
    if response.account_id != args.account
        || response.scope != scope as i32
        || response.dry_run != args.dry_run
        || response.projection.is_none()
        || response.preserved.is_none()
        || response.run.is_some() == args.dry_run
        || response
            .run
            .as_ref()
            .is_some_and(|r| r.account_id != args.account)
    {
        return Err(AppError {
            code: "invalid_repair_response",
            message: "Daemon returned an incomplete or mismatched repair response",
            exit: 1,
            operation: None,
            sync_run: None,
            recovery: None,
        });
    }
    let mut result = crate::mail::json(&response)?;
    if let Some(run) = response.run {
        result["run"] = crate::mail::wait_run(
            &mut SystemClient::with_interceptor(channel, injector),
            run,
            args.wait,
        )
        .await?;
    }
    Ok(result)
}
