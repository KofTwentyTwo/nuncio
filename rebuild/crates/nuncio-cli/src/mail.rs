use crate::{args::MailCommand, output::AppError, rpc_error};
use nuncio_proto::{
    client::TokenInjector,
    v2::{self, mail_client::MailClient, system_client::SystemClient},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{io::Write, path::PathBuf, time::Duration};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
type Transport = InterceptedService<Channel, TokenInjector>;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod action_tests {
    #[test]
    fn action_files_require_version_one_and_reject_unknown_fields_and_oversized_input() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("action.json");
        for invalid in [
            r#"{"action":"archive"}"#,
            r#"{"schema_version":2,"action":"archive"}"#,
            r#"{"schema_version":"1","action":"archive"}"#,
            r#"{"schema_version":1,"action":"archive","unexpected":true}"#,
            r#"{"schema_version":1,"action":"read"}"#,
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(super::read_action(&path).is_err(), "accepted {invalid}");
        }
        std::fs::write(
            &path,
            format!(
                "{{\"schema_version\":1,\"action\":\"archive\"}}{}",
                " ".repeat(8192)
            ),
        )
        .unwrap();
        assert!(super::read_action(&path).is_err());
        std::fs::write(
            &path,
            r#"{"schema_version":1,"action":"read","read":false}"#,
        )
        .unwrap();
        assert!(
            matches!(super::read_action(&path),Ok(nuncio_proto::v2::change_message_request::Action::Read(v)) if !v.read)
        );
    }
}
#[derive(serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ActionFile {
    Read {
        read: bool,
    },
    Star {
        starred: bool,
    },
    Archive {},
    Move {
        destination_collection_id: String,
    },
    Copy {
        destination_collection_id: String,
    },
    Trash {
        trashed: bool,
    },
    Label {
        collection_id: String,
        present: bool,
    },
}
fn read_action(path: &std::path::Path) -> Result<v2::change_message_request::Action, AppError> {
    use std::io::Read;
    use v2::change_message_request::Action;
    let mut bytes = Vec::new();
    crate::drafts::regular_file(path)?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::invalid())?;
    if bytes.len() > 8192 {
        return Err(AppError::invalid());
    }
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| AppError::invalid())?;
    let fields = value.as_object_mut().ok_or_else(AppError::invalid)?;
    if fields
        .remove("schema_version")
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err(AppError::invalid());
    }
    let action: ActionFile = serde_json::from_value(value).map_err(|_| AppError::invalid())?;
    Ok(match action {
        ActionFile::Read { read } => Action::Read(v2::MailReadChange { read }),
        ActionFile::Star { starred } => Action::Star(v2::MailStarChange { starred }),
        ActionFile::Archive {} => Action::Archive(v2::MailArchiveChange {}),
        ActionFile::Move {
            destination_collection_id,
        } => Action::Move(v2::MailFolderChange {
            destination_collection_id,
        }),
        ActionFile::Copy {
            destination_collection_id,
        } => Action::Copy(v2::MailFolderChange {
            destination_collection_id,
        }),
        ActionFile::Trash { trashed } => Action::Trash(v2::MailTrashChange { trashed }),
        ActionFile::Label {
            collection_id,
            present,
        } => Action::Label(v2::MailLabelChange {
            collection_id,
            present,
        }),
    })
}
pub(crate) fn json(value: impl serde::Serialize) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|_| invalid_response())
}
pub async fn run(
    mut command: MailCommand,
    channel: Channel,
    token: TokenInjector,
) -> Result<Value, AppError> {
    if let MailCommand::Fetch {
        account,
        message,
        wait,
    } = command
    {
        return sync(
            &mut SystemClient::with_interceptor(channel, token),
            account,
            false,
            Some(message),
            wait,
        )
        .await;
    }
    if let MailCommand::Raw { output, .. }
    | MailCommand::Body { output, .. }
    | MailCommand::Attachment { output, .. } = &mut command
    {
        let status = SystemClient::with_interceptor(channel.clone(), token.clone())
            .get_status(v2::GetStatusRequest {})
            .await
            .map_err(rpc_error)?
            .into_inner();
        *output =
            crate::output_file::destination(output, &status.protected_paths).map_err(|error| {
                match error {
                    crate::output_file::OutputPathError::Destination => file_error(),
                    crate::output_file::OutputPathError::Protection => invalid_response(),
                }
            })?;
    }
    let mut client = MailClient::with_interceptor(channel.clone(), token.clone());
    match command {
        MailCommand::Capabilities { account } => json(
            client
                .get_capabilities(v2::MailCapabilitiesRequest {
                    account_id: account,
                })
                .await
                .map_err(rpc_error)?
                .into_inner(),
        ),
        MailCommand::Change {
            account,
            message,
            request_id,
            file,
            wait,
        } => {
            let action = read_action(&file)?;
            let op = client
                .change_message(v2::ChangeMessageRequest {
                    account_id: account,
                    message_id: message,
                    request_id,
                    action: Some(action),
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            if wait {
                crate::operations::wait_for(op, channel, token).await
            } else {
                json(op)
            }
        }
        MailCommand::Send {
            account,
            draft,
            request_id,
            version,
            wait,
        } => {
            let operation = client
                .send_draft(v2::SendDraftRequest {
                    account_id: account,
                    draft_id: draft,
                    request_id,
                    expected_version: version,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            if wait {
                crate::operations::wait_for(operation, channel, token).await
            } else {
                json(operation)
            }
        }
        MailCommand::Reply {
            account,
            message,
            body_file,
        } => {
            crate::drafts::prepare(
                &mut client,
                account,
                message,
                "reply",
                Some(body_file),
                vec![],
            )
            .await
        }
        MailCommand::ReplyAll {
            account,
            message,
            body_file,
        } => {
            crate::drafts::prepare(
                &mut client,
                account,
                message,
                "reply_all",
                Some(body_file),
                vec![],
            )
            .await
        }
        MailCommand::Forward {
            account,
            message,
            to,
            body_file,
        } => crate::drafts::prepare(&mut client, account, message, "forward", body_file, to).await,
        MailCommand::Draft { command } => crate::drafts::run(&mut client, command).await,
        MailCommand::List { args } => list(&mut client, args, None).await,
        MailCommand::Search { args, query } => list(&mut client, args, Some(query)).await,
        MailCommand::Collections { account } => json(
            client
                .list_collections(v2::ListCollectionsRequest {
                    account_id: account,
                })
                .await
                .map_err(rpc_error)?
                .into_inner(),
        ),
        MailCommand::Read { account, message } => {
            let detail = client
                .get_message(v2::MessageRequest {
                    account_id: account.clone(),
                    message_id: message.clone(),
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            let mut result = json(detail)?;
            for kind in ["text", "html"] {
                let mut bytes = Vec::new();
                let downloaded = download(
                    &mut client,
                    v2::DownloadMailRequest {
                        account_id: account.clone(),
                        message_id: message.clone(),
                        kind: kind.into(),
                        attachment_id: None,
                    },
                    &mut bytes,
                )
                .await;
                match downloaded {
                    Ok(_) => {
                        result[kind] = Value::String(
                            String::from_utf8(bytes).map_err(|_| invalid_response())?,
                        );
                    }
                    Err(e) if e.code == "not_found" => {
                        result[kind] = Value::Null;
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(result)
        }
        MailCommand::Raw {
            account,
            message,
            output,
        } => save(&mut client, account, message, "raw".into(), None, output).await,
        MailCommand::Body {
            account,
            message,
            kind,
            output,
        } => save(&mut client, account, message, kind, None, output).await,
        MailCommand::Attachment {
            account,
            message,
            attachment,
            output,
        } => {
            save(
                &mut client,
                account,
                message,
                "attachment".into(),
                Some(attachment),
                output,
            )
            .await
        }
        MailCommand::Fetch { .. } => Err(AppError::invalid()),
    }
}
async fn list(
    client: &mut MailClient<Transport>,
    q: crate::args::MailQueryArgs,
    query: Option<String>,
) -> Result<Value, AppError> {
    json(
        client
            .list_messages(v2::ListMailRequest {
                account_id: q.account,
                collection_id: q.collection,
                query,
                page_size: q.page_size,
                page_token: q.page_token,
            })
            .await
            .map_err(rpc_error)?
            .into_inner(),
    )
}
pub async fn sync(
    client: &mut SystemClient<Transport>,
    account: String,
    full: bool,
    fetch: Option<String>,
    wait: bool,
) -> Result<Value, AppError> {
    let run = client
        .start_sync(v2::StartSyncRequest {
            account_id: account,
            full,
            fetch_message_id: fetch,
        })
        .await
        .map_err(rpc_error)?
        .into_inner();
    wait_run(client, run, wait).await
}
pub(crate) async fn wait_run(
    client: &mut SystemClient<Transport>,
    mut run: v2::SyncRun,
    wait: bool,
) -> Result<Value, AppError> {
    if wait {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        loop {
            match run.state.as_str() {
                "succeeded" => break,
                "failed" | "cancelled" => {
                    return Err((match run.error_code.as_deref() {
                        Some("unavailable") => AppError {
                            code: "provider_unavailable",
                            message: "Provider synchronization failed; inspect the sync run",
                            sync_run: None,
                            operation: None,
                            recovery: None,
                            exit: 4,
                        },
                        Some("authorization_required" | "scope_denied") => AppError::auth(),
                        Some("history_expired") => AppError {
                            code: "history_expired",
                            message: "Provider history expired repeatedly; retry synchronization",
                            sync_run: None,
                            operation: None,
                            recovery: None,
                            exit: 4,
                        },
                        _ => AppError {
                            code: "sync_failed",
                            message: "Synchronization did not complete; inspect the sync run",
                            sync_run: None,
                            operation: None,
                            recovery: None,
                            exit: 1,
                        },
                    })
                    .with_sync(&run))
                }
                "queued" | "running" => {}
                _ => return Err(invalid_response()),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AppError {
                    code: "wait_timeout",
                    message: "Synchronization is still pending; inspect the sync run",
                    sync_run: None,
                    operation: None,
                    recovery: None,
                    exit: 4,
                }
                .with_sync(&run));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            run = client
                .get_sync_run(v2::SyncRunRequest {
                    account_id: run.account_id.clone(),
                    run_id: run.id.clone(),
                })
                .await
                .map_err(|error| rpc_error(error).with_sync(&run))?
                .into_inner();
        }
    }
    json(run)
}
async fn save(
    client: &mut MailClient<Transport>,
    account: String,
    message: String,
    kind: String,
    attachment: Option<String>,
    output: PathBuf,
) -> Result<Value, AppError> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| file_error())?;
    let (length, hash) = download(
        client,
        v2::DownloadMailRequest {
            account_id: account,
            message_id: message,
            kind,
            attachment_id: attachment,
        },
        file.as_file_mut(),
    )
    .await?;
    file.as_file().sync_all().map_err(|_| file_error())?;
    file.persist_noclobber(&output).map_err(|_| file_error())?;
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| file_error())?;
    Ok(serde_json::json!({"output":output,"byte_length":length,"sha256":hash}))
}
async fn download(
    client: &mut MailClient<Transport>,
    request: v2::DownloadMailRequest,
    writer: &mut impl Write,
) -> Result<(u64, String), AppError> {
    let mut stream = client
        .download(request)
        .await
        .map_err(rpc_error)?
        .into_inner();
    let mut offset = 0;
    let mut manifest = None;
    let mut hasher = Sha256::new();
    let mut finished = false;
    while let Some(chunk) = stream.message().await.map_err(rpc_error)? {
        if finished
            || chunk.offset != offset
            || chunk.data.len() > 262144
            || chunk.total_bytes > 64 * 1024 * 1024
            || chunk.sha256.len() != 64
        {
            return Err(invalid_response());
        }
        let expected = manifest.get_or_insert((chunk.total_bytes, chunk.sha256.clone()));
        if expected != &(chunk.total_bytes, chunk.sha256)
            || offset + chunk.data.len() as u64 > chunk.total_bytes
        {
            return Err(invalid_response());
        }
        if chunk.data.is_empty() && chunk.total_bytes != 0 {
            return Err(invalid_response());
        }
        hasher.update(&chunk.data);
        writer.write_all(&chunk.data).map_err(|_| file_error())?;
        offset += chunk.data.len() as u64;
        finished = chunk.eof;
    }
    let (total, hash) = manifest.ok_or_else(invalid_response)?;
    if !finished || offset != total || hex::encode(hasher.finalize()) != hash {
        return Err(invalid_response());
    }
    Ok((total, hash))
}
fn invalid_response() -> AppError {
    AppError {
        code: "invalid_response",
        message: "Daemon returned an incomplete or invalid response",
        sync_run: None,
        operation: None,
        recovery: None,
        exit: 1,
    }
}
fn file_error() -> AppError {
    AppError {
        code: "file_io",
        message: "Could not write output; destination must be a new writable path",
        sync_run: None,
        operation: None,
        recovery: None,
        exit: 1,
    }
}
