mod imap;
use nuncio_engine::{
    accounts::{AccountError, GoogleAuthRequest, GoogleRegistration},
    engine::Engine,
};
use nuncio_proto::v2::{self, accounts_server::Accounts};
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub(crate) struct AccountsService(pub Arc<Engine>);
#[tonic::async_trait]
impl Accounts for AccountsService {
    async fn update_imap_account(
        &self,
        request: Request<v2::UpdateImapAccountRequest>,
    ) -> Result<Response<v2::ConnectImapResponse>, Status> {
        let input = request.into_inner();
        let config = imap::config(input.config.ok_or_else(|| error(AccountError::Invalid))?)?;
        let credentials = input.credentials.map(|mut value| {
            nuncio_engine::domain::imap_account::ImapCredentials {
                imap_password: std::mem::take(&mut value.imap_password),
                smtp_password: std::mem::take(&mut value.smtp_password),
            }
        });
        let result = self
            .0
            .update_imap_account(input.account_id, input.version, config, credentials)
            .await
            .map_err(error)?;
        Ok(Response::new(v2::ConnectImapResponse {
            account: Some(account(result.account)),
            capabilities: Some(imap::capabilities(result.capabilities)),
            credential_cleanup_pending: result.credential_cleanup_pending,
        }))
    }
    async fn connect_imap(
        &self,
        request: Request<v2::ConnectImapRequest>,
    ) -> Result<Response<v2::ConnectImapResponse>, Status> {
        let input = request.into_inner();
        let config = imap::config(input.config.ok_or_else(|| error(AccountError::Invalid))?)?;
        let mut credentials = input
            .credentials
            .ok_or_else(|| error(AccountError::Invalid))?;
        let result = self
            .0
            .connect_imap(
                config,
                nuncio_engine::domain::imap_account::ImapCredentials {
                    imap_password: std::mem::take(&mut credentials.imap_password),
                    smtp_password: std::mem::take(&mut credentials.smtp_password),
                },
                input.account_id,
            )
            .await
            .map_err(error)?;
        Ok(Response::new(v2::ConnectImapResponse {
            account: Some(account(result.account)),
            capabilities: Some(imap::capabilities(result.capabilities)),
            credential_cleanup_pending: result.credential_cleanup_pending,
        }))
    }
    async fn get_imap_config(
        &self,
        request: Request<v2::AccountRequest>,
    ) -> Result<Response<v2::ImapConfigResponse>, Status> {
        let (config, caps) = self
            .0
            .imap_config(request.into_inner().account_id)
            .await
            .map_err(error)?;
        Ok(Response::new(v2::ImapConfigResponse {
            config: Some(imap::configuration(config)),
            capabilities: Some(imap::capabilities(caps)),
        }))
    }

    async fn begin_google_auth(
        &self,
        request: Request<v2::BeginGoogleAuthRequest>,
    ) -> Result<Response<v2::AuthSession>, Status> {
        let mut request = request.into_inner();
        let result = self
            .0
            .begin_google_auth(GoogleAuthRequest {
                registration: GoogleRegistration {
                    client_id: std::mem::take(&mut request.client_id),
                    client_secret: request.client_secret.take(),
                },
                login_hint: request.login_hint.take(),
                account: request.account_id.take(),
            })
            .await
            .map_err(error)?;
        Ok(Response::new(session(result)))
    }
    async fn get_auth_status(
        &self,
        request: Request<v2::GetAuthStatusRequest>,
    ) -> Result<Response<v2::AuthSession>, Status> {
        Ok(Response::new(session(
            self.0
                .auth_status(&request.into_inner().session_id)
                .await
                .map_err(error)?,
        )))
    }
    async fn list_accounts(
        &self,
        request: Request<v2::ListAccountsRequest>,
    ) -> Result<Response<v2::ListAccountsResponse>, Status> {
        Ok(Response::new(v2::ListAccountsResponse {
            accounts: self
                .0
                .list_accounts_including_archived(request.into_inner().include_archived)
                .await
                .map_err(error)?
                .into_iter()
                .map(account)
                .collect(),
        }))
    }
    async fn disconnect_account(
        &self,
        request: Request<v2::AccountRequest>,
    ) -> Result<Response<v2::DisconnectAccountResponse>, Status> {
        self.0
            .disconnect_account(&request.into_inner().account_id)
            .await
            .map_err(error)?;
        Ok(Response::new(v2::DisconnectAccountResponse {}))
    }
    async fn check_account(
        &self,
        request: Request<v2::AccountRequest>,
    ) -> Result<Response<v2::Account>, Status> {
        Ok(Response::new(account(
            self.0
                .check_account(&request.into_inner().account_id)
                .await
                .map_err(error)?,
        )))
    }
    async fn get_account(
        &self,
        request: Request<v2::AccountRequest>,
    ) -> Result<Response<v2::Account>, Status> {
        Ok(Response::new(account(
            self.0
                .show_account(&request.into_inner().account_id)
                .await
                .map_err(error)?,
        )))
    }
    async fn update_account(
        &self,
        request: Request<v2::UpdateAccountRequest>,
    ) -> Result<Response<v2::Account>, Status> {
        let input = request.into_inner();
        Ok(Response::new(account(
            self.0
                .edit_account_name(&input.account_id, input.version, input.display_name)
                .await
                .map_err(error)?,
        )))
    }
    async fn set_account_lifecycle(
        &self,
        request: Request<v2::SetAccountLifecycleRequest>,
    ) -> Result<Response<v2::AccountLifecycleResponse>, Status> {
        use nuncio_engine::store::AccountLifecycle;
        let input = request.into_inner();
        let action = match input.action() {
            v2::AccountLifecycleAction::Pause => AccountLifecycle::Pause,
            v2::AccountLifecycleAction::Resume => AccountLifecycle::Resume,
            v2::AccountLifecycleAction::Archive => AccountLifecycle::Archive,
            v2::AccountLifecycleAction::Restore => AccountLifecycle::Restore,
            v2::AccountLifecycleAction::Unspecified => return Err(error(AccountError::Invalid)),
        };
        let (value, pending) = self
            .0
            .change_account_lifecycle(&input.account_id, action)
            .await
            .map_err(error)?;
        Ok(Response::new(v2::AccountLifecycleResponse {
            account: Some(account(value)),
            credential_cleanup_pending: pending,
        }))
    }
    async fn preview_account_purge(
        &self,
        request: Request<v2::AccountRequest>,
    ) -> Result<Response<v2::AccountPurgePreview>, Status> {
        Ok(Response::new(preview(
            self.0
                .preview_account_purge(request.into_inner().account_id)
                .await
                .map_err(error)?,
        )))
    }
    async fn purge_account(
        &self,
        request: Request<v2::PurgeAccountRequest>,
    ) -> Result<Response<v2::PurgeAccountResponse>, Status> {
        let input = request.into_inner();
        if input.confirm_account_id != input.account_id {
            return Err(error(AccountError::Invalid));
        }
        let (deleted, pending) = self
            .0
            .purge_account(&input.account_id, input.version, input.revision)
            .await
            .map_err(error)?;
        Ok(Response::new(v2::PurgeAccountResponse {
            deleted: Some(preview(deleted)),
            credential_cleanup_pending: pending,
        }))
    }
    async fn cancel_google_auth(
        &self,
        request: Request<v2::GetAuthStatusRequest>,
    ) -> Result<Response<v2::AuthSession>, Status> {
        Ok(Response::new(session(
            self.0
                .cancel_google_auth(&request.into_inner().session_id)
                .await
                .map_err(error)?,
        )))
    }
}
fn preview(value: nuncio_engine::store::AccountPurgePreview) -> v2::AccountPurgePreview {
    v2::AccountPurgePreview {
        account_id: value.account_id,
        version: value.version,
        revision: value.revision,
        archived: value.archived,
        messages: value.messages,
        calendars: value.calendars,
        events: value.events,
        drafts: value.drafts,
        blobs: value.blobs,
        blob_bytes: value.blob_bytes,
        operations: value.operations,
        queued_operations: value.queued_operations,
        unresolved_operations: value.unresolved_operations,
    }
}
fn account(value: nuncio_engine::accounts::Account) -> v2::Account {
    v2::Account {
        id: value.id,
        provider: value.provider,
        address: value.address,
        state: value.state,
        display_name: value.display_name,
        version: value.version,
        auth_state: value.auth_state,
        identity: value.identity,
    }
}
fn session(value: nuncio_engine::accounts::AuthSession) -> v2::AuthSession {
    v2::AuthSession {
        session_id: value.session_id,
        browser_url: value.browser_url,
        expires_at_ms: value.expires_at_ms,
        state: value.state,
        account_id: value.account_id,
        error_code: value.error_code,
        warning_code: value.warning_code,
    }
}
fn error(error: AccountError) -> Status {
    let message = error.to_string();
    match error {
        AccountError::VersionConflict => Status::aborted(message),
        AccountError::Lifecycle => Status::failed_precondition(message),
        AccountError::Invalid => Status::invalid_argument(message),
        AccountError::NotFound => Status::not_found(message),
        AccountError::Authorization | AccountError::ScopeDenied => Status::unauthenticated(message),
        AccountError::IdentityMismatch | AccountError::Tls | AccountError::Unsupported => {
            Status::failed_precondition(message)
        }
        AccountError::Limit => Status::resource_exhausted(message),
        AccountError::Unavailable | AccountError::RetryAfter(_) => Status::unavailable(message),
        _ => Status::internal(message),
    }
}
