use crate::{args::DraftCommand, mail::json, output::AppError, rpc_error};
use nuncio_proto::{
    client::TokenInjector,
    v2::{self, mail_client::MailClient},
};
use std::io::Read;
use tonic::{service::interceptor::InterceptedService, transport::Channel};

pub async fn run(
    client: &mut MailClient<InterceptedService<Channel, TokenInjector>>,
    command: DraftCommand,
) -> Result<serde_json::Value, AppError> {
    match command {
        DraftCommand::Attach {
            account,
            draft,
            version,
            file,
            mime_type,
            charset,
            content_id,
            inline,
        } => {
            let (stream, failed) = crate::draft_upload::prepare(
                &file,
                v2::DraftUploadHeader {
                    account_id: account,
                    draft_id: draft,
                    expected_version: version,
                    filename: file
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned),
                    mime_type,
                    parameters: charset.map(|s| ("charset".into(), s)).into_iter().collect(),
                    content_id,
                    disposition: if inline { "inline" } else { "attachment" }.into(),
                    byte_length: 0,
                    sha256: String::new(),
                },
            )?;
            let result = client.upload_draft_attachment(stream).await;
            if failed.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(file_error());
            }
            json(result.map_err(rpc_error)?.into_inner())
        }
        DraftCommand::Save {
            account,
            draft,
            version,
            file,
        } => {
            let mut bytes = Vec::new();
            regular_file(&file)?
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| file_error())?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err(AppError::invalid());
            }
            let content: v2::DraftContent =
                serde_json::from_slice(&bytes).map_err(|_| AppError::invalid())?;
            json(
                client
                    .save_draft(v2::SaveDraftRequest {
                        account_id: account,
                        draft_id: draft,
                        expected_version: version,
                        content: Some(content),
                    })
                    .await
                    .map_err(rpc_error)?
                    .into_inner(),
            )
        }
        DraftCommand::Show { account, draft } => json(
            client
                .get_draft(v2::DraftRequest {
                    account_id: account,
                    draft_id: draft,
                })
                .await
                .map_err(rpc_error)?
                .into_inner(),
        ),
        DraftCommand::List {
            account,
            page_size,
            page_token,
        } => json(
            client
                .list_drafts(v2::ListDraftsRequest {
                    account_id: account,
                    page_size,
                    page_token,
                })
                .await
                .map_err(rpc_error)?
                .into_inner(),
        ),
        DraftCommand::Delete {
            account,
            draft,
            version,
        } => json(
            client
                .delete_draft(v2::DeleteDraftRequest {
                    account_id: account,
                    draft_id: draft,
                    expected_version: version,
                })
                .await
                .map_err(rpc_error)?
                .into_inner(),
        ),
    }
}
pub(crate) async fn prepare(
    client: &mut MailClient<InterceptedService<Channel, TokenInjector>>,
    account: String,
    message: String,
    kind: &str,
    body_file: Option<std::path::PathBuf>,
    to: Vec<String>,
) -> Result<serde_json::Value, AppError> {
    let body = body_file
        .map(|path| {
            let mut bytes = Vec::new();
            regular_file(&path)?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| file_error())?;
            if bytes.len() > 1024 * 1024 {
                return Err(AppError::invalid());
            }
            String::from_utf8(bytes).map_err(|_| AppError::invalid())
        })
        .transpose()?;
    json(
        client
            .prepare_draft(v2::PrepareDraftRequest {
                account_id: account,
                message_id: message,
                kind: kind.into(),
                body,
                to: to
                    .into_iter()
                    .map(|address| v2::DraftRecipient {
                        address,
                        name: None,
                    })
                    .collect(),
            })
            .await
            .map_err(rpc_error)?
            .into_inner(),
    )
}
pub(crate) fn regular_file(path: &std::path::Path) -> Result<std::fs::File, AppError> {
    if !std::fs::metadata(path).map_err(|_| file_error())?.is_file() {
        return Err(file_error());
    }
    let file = std::fs::File::open(path).map_err(|_| file_error())?;
    if !file.metadata().map_err(|_| file_error())?.is_file() {
        return Err(file_error());
    }
    Ok(file)
}
pub(crate) fn file_error() -> AppError {
    AppError {
        code: "file_io",
        message: "Unable to read a stable regular draft or attachment file",
        exit: 1,
        sync_run: None,
        operation: None,
        recovery: None,
    }
}
