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
        _: Request<v2::ListAccountsRequest>,
    ) -> Result<Response<v2::ListAccountsResponse>, Status> {
        Ok(Response::new(v2::ListAccountsResponse {
            accounts: self
                .0
                .list_accounts()
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
}
fn account(value: nuncio_engine::accounts::Account) -> v2::Account {
    v2::Account {
        id: value.id,
        provider: value.provider,
        address: value.address,
        state: value.state,
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
