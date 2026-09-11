use crate::{args::OperationsCommand, mail::json, output::AppError, rpc_error};
use nuncio_proto::{
    client::TokenInjector,
    v2::{self, operations_client::OperationsClient, OperationRequest},
};
use tonic::transport::Channel;
pub(crate) async fn wait_for(
    mut operation: v2::Operation,
    channel: Channel,
    token: TokenInjector,
) -> Result<serde_json::Value, AppError> {
    let mut client = OperationsClient::with_interceptor(channel.clone(), token.clone())
        .max_decoding_message_size(8 * 1024 * 1024);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match operation.state.as_str() {
            "applied"=>return json(operation),
            "failed"=>return Err(AppError{code:"operation_failed",message:"Provider rejected the operation; inspect its attempts",exit:1,sync_run:None,operation: None, recovery: None}.with_operation(&operation)),
            "conflict"|"cancelled"=>return Err(AppError{code:"operation_not_applied",message:"Operation was cancelled or has a conflict",exit:5,sync_run:None,operation: None, recovery: None}.with_operation(&operation)),
            "uncertain" if !operation.needs_reconciliation=>return Err(AppError{code:"operation_uncertain",message:"Provider outcome remains uncertain; inspect the operation before deciding what to do",exit:5,sync_run:None,operation: None, recovery: None}.with_operation(&operation)),
            "queued"|"running"|"retry_wait"|"uncertain"=>{},
            _=>return Err(AppError::unavailable().with_operation(&operation)),
        }
        let request = OperationRequest {
            account_id: operation.account_id.clone(),
            operation_id: operation.id.clone(),
        };
        tokio::select! {
            _=tokio::time::sleep_until(deadline)=>return Err(AppError{code:"operation_wait_timeout",message:"Wait timed out; the durable operation remains inspectable",exit:4,sync_run:None,operation: None, recovery: None}.with_operation(&operation)),
            result=async {tokio::time::sleep(std::time::Duration::from_millis(100)).await;client.get_operation(request).await}=>{
                operation=result.map_err(|e|rpc_error(e).with_operation(&operation))?.into_inner();
            }
        }
    }
}
pub(crate) async fn run(
    command: OperationsCommand,
    channel: Channel,
    token: TokenInjector,
) -> Result<serde_json::Value, AppError> {
    let mut client = OperationsClient::with_interceptor(channel.clone(), token.clone())
        .max_decoding_message_size(8 * 1024 * 1024);
    match command {
        OperationsCommand::List {
            account,
            page_size,
            page_token,
        } => {
            return json(
                client
                    .list_operations(v2::ListOperationsRequest {
                        account_id: account,
                        page_size,
                        page_token,
                    })
                    .await
                    .map_err(rpc_error)?
                    .into_inner(),
            )
        }
        OperationsCommand::Attempts {
            account,
            operation,
            page_size,
            page_token,
        } => {
            return json(
                client
                    .list_attempts(v2::ListOperationAttemptsRequest {
                        account_id: account,
                        operation_id: operation,
                        page_size,
                        page_token,
                    })
                    .await
                    .map_err(rpc_error)?
                    .into_inner(),
            )
        }
        _ => {}
    }
    let result = match command {
        OperationsCommand::Reconcile {
            account,
            operation,
            request_id,
            version,
            resume_safe,
            wait,
        } => {
            let operation = client
                .reconcile_operation(v2::ReconcileOperationRequest {
                    account_id: account,
                    operation_id: operation,
                    request_id,
                    expected_version: version,
                    mode: if resume_safe {
                        v2::ReconciliationMode::ResumeSafe
                    } else {
                        v2::ReconciliationMode::Observe
                    } as i32,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            return if wait {
                wait_for(operation, channel, token).await
            } else {
                json(operation)
            };
        }
        OperationsCommand::Wait { account, operation } => {
            let operation = client
                .get_operation(OperationRequest {
                    account_id: account,
                    operation_id: operation,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            return wait_for(operation, channel, token).await;
        }
        OperationsCommand::List { .. } | OperationsCommand::Attempts { .. } => {
            return Err(AppError::invalid())
        }
        OperationsCommand::Show { account, operation } => {
            client
                .get_operation(OperationRequest {
                    account_id: account,
                    operation_id: operation,
                })
                .await
        }
        OperationsCommand::Cancel { account, operation } => {
            client
                .cancel_operation(OperationRequest {
                    account_id: account,
                    operation_id: operation,
                })
                .await
        }
        OperationsCommand::Resolve {
            account,
            operation,
            version,
            file,
            accept_duplicate_risk,
        } => {
            let decision = read_decision(&file, accept_duplicate_risk)?;
            client
                .resolve_operation(v2::ResolveOperationRequest {
                    account_id: account,
                    operation_id: operation,
                    expected_version: version,
                    decision: Some(decision),
                })
                .await
        }
    }
    .map_err(rpc_error)?
    .into_inner();
    json(result)
}

#[derive(serde::Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
enum DecisionFile {
    Abandon { reason: String },
    ConfirmApplied { evidence: v2::OperationConfirmation },
    Resend { request_id: String, reason: String },
}
fn read_decision(
    path: &std::path::Path,
    accept_duplicate_risk: bool,
) -> Result<v2::resolve_operation_request::Decision, AppError> {
    use std::io::{Read, Write};
    use v2::resolve_operation_request::Decision;
    let mut bytes = Vec::new();
    crate::drafts::regular_file(path)?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| crate::drafts::file_error())?;
    if bytes.len() > 16 * 1024 {
        return Err(AppError::invalid());
    }
    let decision: DecisionFile = serde_json::from_slice(&bytes).map_err(|_| AppError::invalid())?;
    if accept_duplicate_risk && !matches!(decision, DecisionFile::Resend { .. }) {
        return Err(AppError::invalid());
    }
    Ok(match decision {
        DecisionFile::Abandon { reason } => Decision::Abandon(v2::OperationAbandonment { reason }),
        DecisionFile::ConfirmApplied { evidence } => Decision::ConfirmApplied(evidence),
        DecisionFile::Resend { request_id, reason } => {
            if !accept_duplicate_risk {
                return Err(AppError{code:"duplicate_risk_not_accepted",message:"Resend may duplicate a message already delivered. Explicit --accept-duplicate-risk is required.",exit:2,sync_run:None,operation: None, recovery: None});
            }
            writeln!(std::io::stderr().lock(),"Resend may duplicate a message already delivered; creating a separate send request.").map_err(|_|AppError::unavailable())?;
            Decision::Resend(v2::OperationResend {
                request_id,
                reason,
                accept_duplicate_risk: true,
            })
        }
    })
}
