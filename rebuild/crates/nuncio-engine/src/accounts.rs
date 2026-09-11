mod imap;
use crate::{
    providers::google::{
        http::{GoogleApi, GoogleHttp, GoogleRead, TokenResponse, UserInfo},
        oauth,
    },
    secrets::SecretStore,
    store::{ConnectedAccount, Store, StoredAccount},
};
pub use imap::ImapConnection;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{watch, Mutex};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

enum GoogleWrite<'a> {
    Gmail {
        sender: Option<&'a str>,
        path: &'a [&'a str],
        body: Vec<u8>,
    },
    Calendar(&'a crate::store::CalendarChangePayload),
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Mail server TLS verification failed")]
    Tls,
    #[error("Mail server does not support a required protocol capability")]
    Unsupported,
    #[error("Invalid account configuration")]
    Invalid,
    #[error("Account or authorization session was not found")]
    NotFound,
    #[error("Account authorization is required")]
    Authorization,
    #[error("Required Google permissions were not granted")]
    ScopeDenied,
    #[error("Provider account identity does not match the saved account")]
    IdentityMismatch,
    #[error("Provider service is unavailable")]
    Unavailable,
    #[error("Provider service requested a later attempt")]
    RetryAfter(i64),
    #[error("Provider returned an invalid response")]
    Provider,
    #[error("Secure credential storage is unavailable")]
    Secret,
    #[error("Account storage is unavailable")]
    Storage,
    #[error("Too many pending authorization sessions or accounts")]
    Limit,
}
impl AccountError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Tls => "tls_verification",
            Self::Unsupported => "unsupported",
            Self::Invalid => "invalid_input",
            Self::NotFound => "not_found",
            Self::Authorization => "authorization_required",
            Self::ScopeDenied => "scope_denied",
            Self::IdentityMismatch => "identity_mismatch",
            Self::Unavailable | Self::RetryAfter(_) => "unavailable",
            Self::Provider => "invalid_provider_response",
            Self::Secret => "credential_storage",
            Self::Storage => "storage",
            Self::Limit => "limit",
        }
    }
}
impl From<crate::store::StoreError> for AccountError {
    fn from(_: crate::store::StoreError) -> Self {
        Self::Storage
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct GoogleRegistration {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}
impl GoogleRegistration {
    fn validate(&self, synthetic: bool) -> Result<(), AccountError> {
        let valid_id = if synthetic {
            self.client_id == "nuncio-test-client"
        } else {
            self.client_id.ends_with(".apps.googleusercontent.com")
                && self.client_id.len() <= 256
                && self
                    .client_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        };
        if !valid_id
            || self.client_secret.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 4096
                    || s.chars().any(char::is_control)
                    || synthetic && s != "synthetic-client-secret"
            })
        {
            return Err(AccountError::Invalid);
        }
        Ok(())
    }
}
pub struct GoogleAuthRequest {
    pub registration: GoogleRegistration,
    pub login_hint: Option<String>,
    pub account: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct AuthSession {
    pub session_id: String,
    pub browser_url: String,
    pub expires_at_ms: i64,
    pub state: String,
    pub account_id: Option<String>,
    pub error_code: Option<String>,
    pub warning_code: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub address: String,
    pub state: String,
}
impl From<StoredAccount> for Account {
    fn from(row: StoredAccount) -> Self {
        Self {
            id: row.account.id,
            provider: row.account.provider,
            address: row.account.address,
            state: row.state,
        }
    }
}
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub(crate) struct Credentials {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub refresh_token: String,
}
struct Access {
    token: Zeroizing<String>,
    expires: Instant,
}
pub(crate) struct SessionSlot {
    pub status: AuthSession,
    pub expected_account: Option<String>,
    pub cancel: watch::Sender<bool>,
}
pub(crate) struct Accounts {
    store: Store,
    profile: String,
    secrets: Arc<dyn SecretStore>,
    pub http: GoogleHttp,
    pub sessions: Mutex<BTreeMap<String, SessionSlot>>,
    gates: Mutex<BTreeMap<String, Arc<Mutex<Option<Access>>>>>,
    mutations: Mutex<()>,
    pub stop: watch::Sender<bool>,
    pub tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
impl Accounts {
    pub async fn new(
        store: Store,
        profile: String,
        secrets: Arc<dyn SecretStore>,
        http: GoogleHttp,
    ) -> Result<Arc<Self>, AccountError> {
        let service = Arc::new(Self {
            store,
            profile,
            secrets,
            http,
            sessions: Mutex::new(BTreeMap::new()),
            gates: Mutex::new(BTreeMap::new()),
            mutations: Mutex::new(()),
            stop: watch::channel(false).0,
            tasks: Mutex::new(Vec::new()),
        });
        service.cleanup().await?;
        Ok(service)
    }
    pub async fn begin(
        self: &Arc<Self>,
        request: GoogleAuthRequest,
    ) -> Result<AuthSession, AccountError> {
        request.registration.validate(self.http.synthetic)?;
        if request
            .login_hint
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 320 || s.chars().any(char::is_control))
        {
            return Err(AccountError::Invalid);
        }
        if let Some(id) = &request.account {
            let row = self.row(id).await?;
            if row.account.provider != "google" || row.subject.is_none() {
                return Err(AccountError::Invalid);
            }
        }
        oauth::begin(self.clone(), request).await
    }
    pub async fn auth_status(&self, id: &str) -> Result<AuthSession, AccountError> {
        validate_id(id)?;
        self.sessions
            .lock()
            .await
            .get(id)
            .map(|s| s.status.clone())
            .ok_or(AccountError::NotFound)
    }
    pub async fn list(&self) -> Result<Vec<Account>, AccountError> {
        let mut result = Vec::new();
        for row in self.store.accounts().await? {
            result.push(self.row(&row.id).await?.into());
        }
        Ok(result)
    }
    pub async fn disconnect(&self, id: &str) -> Result<(), AccountError> {
        self.row(id).await?;
        let gate = self.gate(id).await;
        let mut access = gate.lock().await;
        let _mutation = self.mutations.lock().await;
        self.row(id).await?;
        for slot in self.sessions.lock().await.values_mut() {
            if slot.expected_account.as_deref() == Some(id)
                && matches!(slot.status.state.as_str(), "pending" | "completing")
            {
                slot.status.state = "cancelled".into();
                slot.status.browser_url.clear();
                slot.cancel.send_replace(true);
            }
        }
        self.store
            .set_account_state(id.into(), "disconnected".into())
            .await?;
        *access = None;
        self.cleanup().await
    }
    pub async fn check(&self, id: &str) -> Result<Account, AccountError> {
        self.row(id).await?;
        let gate = self.gate(id).await;
        let mut access = gate.lock().await;
        let row = self.row(id).await?;
        if row.account.provider == "imap" {
            return self.check_imap(row).await;
        }
        if row.account.provider != "google" {
            return Err(AccountError::Invalid);
        }
        if row.state != "connected" {
            return Err(AccountError::Authorization);
        }
        for scope in ["gmail", "calendar"] {
            if let Some(deadline) = self
                .store
                .provider_retry_deadline(id.into(), scope.into())
                .await?
            {
                if deadline > self.http.clock.now_ms() {
                    return Err(AccountError::RetryAfter(deadline));
                }
            }
        }
        let reference = row
            .credential_ref
            .as_ref()
            .ok_or(AccountError::Authorization)?;
        let result = async {
            if access.as_ref().is_none_or(|a| a.expires <= Instant::now()) {
                *access = Some(self.refresh(reference).await?);
            }
            let current = access.as_ref().ok_or(AccountError::Authorization)?;
            let info = match self.http.userinfo(&current.token).await {
                Err(AccountError::Authorization) => {
                    *access = Some(self.refresh(reference).await?);
                    self.http
                        .userinfo(&access.as_ref().ok_or(AccountError::Authorization)?.token)
                        .await?
                }
                other => other?,
            };
            if row.subject.as_deref() != Some(&info.sub) {
                return Err(AccountError::IdentityMismatch);
            }
            Ok(())
        }
        .await;
        if matches!(
            result,
            Err(AccountError::Authorization
                | AccountError::ScopeDenied
                | AccountError::IdentityMismatch)
        ) {
            let _mutation = self.mutations.lock().await;
            self.store
                .set_account_state(id.into(), "needs_auth".into())
                .await?;
            *access = None;
            self.cleanup().await?;
        }
        if let Err(AccountError::RetryAfter(deadline)) = &result {
            self.auth_retry_after(id, *deadline).await?;
        }
        result?;
        Ok(row.into())
    }
    async fn refresh(&self, reference: &str) -> Result<Access, AccountError> {
        let bytes = self
            .secret_get(reference)
            .await?
            .ok_or(AccountError::Authorization)?;
        let mut credentials: Credentials =
            serde_json::from_slice(&bytes).map_err(|_| AccountError::Secret)?;
        let response = self.http.refresh(&credentials).await?;
        if let Some(refresh) = &response.refresh_token {
            credentials.refresh_token.zeroize();
            credentials.refresh_token = refresh.clone();
            self.secret_put(
                reference,
                Zeroizing::new(serde_json::to_vec(&credentials).map_err(|_| AccountError::Secret)?),
            )
            .await?;
        }
        Ok(access(&response))
    }
    pub async fn gmail_get(
        &self,
        id: &str,
        path: &[&str],
        query: &[(&str, &str)],
        limit: usize,
    ) -> Result<serde_json::Value, crate::mail::MailError> {
        self.google_get(id, GoogleApi::Gmail, path, query, limit)
            .await
    }
    pub async fn gmail_write(
        &self,
        id: &str,
        sender: Option<&str>,
        path: &[&str],
        body: Vec<u8>,
        retry_at_ms: i64,
    ) -> Result<crate::providers::google::send::WriteResult, AccountError> {
        self.google_write(id, GoogleWrite::Gmail { sender, path, body }, retry_at_ms)
            .await
    }
    pub async fn calendar_write(
        &self,
        id: &str,
        intent: &crate::store::CalendarChangePayload,
        retry_at_ms: i64,
    ) -> Result<crate::providers::google::send::WriteResult, AccountError> {
        self.google_write(id, GoogleWrite::Calendar(intent), retry_at_ms)
            .await
    }
    async fn google_write(
        &self,
        id: &str,
        request: GoogleWrite<'_>,
        retry_at_ms: i64,
    ) -> Result<crate::providers::google::send::WriteResult, AccountError> {
        use crate::providers::google::send::WriteResult;
        let (api, sender) = match &request {
            GoogleWrite::Gmail { sender, .. } => (GoogleApi::Gmail, *sender),
            GoogleWrite::Calendar(_) => (GoogleApi::Calendar, None),
        };
        let gate = self.gate(id).await;
        let mut cached = gate.lock().await;
        let row = self.row(id).await?;
        if row.account.provider != "google" {
            return Err(AccountError::Invalid);
        }
        if sender.is_some_and(|sender| !row.account.address.eq_ignore_ascii_case(sender)) {
            return Err(AccountError::IdentityMismatch);
        }
        if row.state != "connected" {
            return Err(AccountError::Authorization);
        }
        if let Some(at) = self
            .store
            .provider_retry_deadline(id.into(), api.scope().into())
            .await?
        {
            if at > self.http.clock.now_ms() {
                return Err(AccountError::RetryAfter(at));
            }
        }
        let reference = row
            .credential_ref
            .as_deref()
            .ok_or(AccountError::Authorization)?;
        let mut result = async {
            if cached.as_ref().is_none_or(|a| a.expires <= Instant::now()) {
                *cached = Some(self.refresh(reference).await?);
            }
            let current = cached.as_ref().ok_or(AccountError::Authorization)?;
            Ok::<_, AccountError>(match request {
                GoogleWrite::Gmail { path, body, .. } => {
                    self.http
                        .write_gmail(&current.token, path, body, retry_at_ms)
                        .await
                }
                GoogleWrite::Calendar(intent) => {
                    self.http
                        .write_calendar(&current.token, intent, retry_at_ms)
                        .await
                }
            })
        }
        .await;
        if let Ok(WriteResult::Rejected {
            code: "authorization_required",
            retry_at_ms,
            ..
        }) = &result
        {
            let retry_at_ms = *retry_at_ms;
            result = match self.refresh(reference).await {
                Ok(access) => {
                    *cached = Some(access);
                    Ok(WriteResult::Rejected {
                        code: "access_token_rejected",
                        retry_at_ms,
                        authorization: false,
                    })
                }
                Err(error) => Err(error),
            };
        }
        if matches!(
            &result,
            Ok(WriteResult::Rejected {
                authorization: true,
                ..
            }) | Err(AccountError::Authorization | AccountError::ScopeDenied)
        ) {
            let _mutation = self.mutations.lock().await;
            self.store
                .set_account_state(id.into(), "needs_auth".into())
                .await?;
            *cached = None;
        }
        match &result {
            Ok(outcome) => {
                if let Some(at) = outcome.retry_deadline() {
                    if self
                        .store
                        .provider_retry_after(id.into(), api.scope().into(), at)
                        .await
                        .is_err()
                    {
                        return Ok(WriteResult::Uncertain {
                            code: "post_submission_storage_failed",
                            retry_at_ms: Some(at),
                        });
                    }
                }
            }
            Err(AccountError::RetryAfter(at)) => self.auth_retry_after(id, *at).await?,
            _ => {}
        }
        result
    }
    pub async fn calendar_get(
        &self,
        id: &str,
        path: &[&str],
        query: &[(&str, &str)],
        limit: usize,
    ) -> Result<serde_json::Value, crate::sync_error::SyncError> {
        self.google_get(id, GoogleApi::Calendar, path, query, limit)
            .await
    }
    async fn google_get(
        &self,
        id: &str,
        api: GoogleApi,
        path: &[&str],
        query: &[(&str, &str)],
        limit: usize,
    ) -> Result<serde_json::Value, crate::sync_error::SyncError> {
        self.google_read(id, api, GoogleRead::Get { path, query, limit })
            .await
    }
    pub async fn calendar_free_busy(
        &self,
        id: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, crate::sync_error::SyncError> {
        self.google_read(id, GoogleApi::Calendar, GoogleRead::FreeBusy(body))
            .await
    }
    async fn google_read(
        &self,
        id: &str,
        api: GoogleApi,
        input: GoogleRead<'_>,
    ) -> Result<serde_json::Value, crate::sync_error::SyncError> {
        use crate::mail::MailError;
        self.row(id).await?;
        let gate = self.gate(id).await;
        let mut access = gate.lock().await;
        let row = self.row(id).await?;
        if row.account.provider != "google" {
            return Err(AccountError::Invalid.into());
        }
        if row.state != "connected" {
            return Err(AccountError::Authorization.into());
        }
        if let Some(deadline) = self
            .store
            .provider_retry_deadline(id.into(), api.scope().into())
            .await?
        {
            if deadline > self.http.clock.now_ms() {
                return Err(MailError::RetryAfter(deadline));
            }
        }
        let reference = row
            .credential_ref
            .as_ref()
            .ok_or(AccountError::Authorization)?;
        let result = async {
            if access.as_ref().is_none_or(|a| a.expires <= Instant::now()) {
                *access = Some(self.refresh(reference).await?);
            }
            let current = access.as_ref().ok_or(AccountError::Authorization)?;
            match self.http.read(api, &current.token, input).await {
                Err(MailError::Account(AccountError::Authorization)) => {
                    *access = Some(self.refresh(reference).await?);
                    self.http
                        .read(
                            api,
                            &access.as_ref().ok_or(AccountError::Authorization)?.token,
                            input,
                        )
                        .await
                }
                other => other,
            }
        }
        .await;
        if matches!(
            result,
            Err(MailError::Account(
                AccountError::Authorization | AccountError::ScopeDenied
            ))
        ) {
            let _mutation = self.mutations.lock().await;
            self.store
                .set_account_state(id.into(), "needs_auth".into())
                .await?;
            *access = None;
            self.cleanup().await?;
        }
        match &result {
            Err(MailError::RetryAfter(deadline)) => {
                self.store
                    .provider_retry_after(id.into(), api.scope().into(), *deadline)
                    .await?
            }
            Err(MailError::Account(AccountError::RetryAfter(deadline))) => {
                self.auth_retry_after(id, *deadline).await?
            }
            _ => {}
        }
        result
    }
    async fn auth_retry_after(&self, id: &str, deadline: i64) -> Result<(), AccountError> {
        // OAuth is shared by both APIs; a token endpoint throttle applies to both.
        for scope in ["gmail", "calendar"] {
            self.store
                .provider_retry_after(id.into(), scope.into(), deadline)
                .await?;
        }
        Ok(())
    }
    pub async fn finish(
        &self,
        session: &str,
        request: &GoogleAuthRequest,
        info: UserInfo,
        token: TokenResponse,
    ) -> Result<(String, bool), AccountError> {
        let id = if let Some(id) = &request.account {
            id.clone()
        } else {
            let mut found = None;
            for row in self.store.accounts().await? {
                let row = self.row(&row.id).await?;
                if row.account.provider == "google" && row.subject.as_deref() == Some(&info.sub) {
                    found = Some(row.account.id);
                    break;
                }
            }
            found.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        };
        let gate = self.gate(&id).await;
        let mut cached = gate.lock().await;
        let _mutation = self.mutations.lock().await;
        if self
            .sessions
            .lock()
            .await
            .get(session)
            .is_none_or(|s| s.status.state != "completing")
        {
            return Err(AccountError::Authorization);
        }
        if let Some(row) = self.store.account(id.clone()).await? {
            if row.account.provider != "google" || row.subject.as_deref() != Some(&info.sub) {
                return Err(AccountError::IdentityMismatch);
            }
        } else if self.store.status().await?.account_count >= 100 {
            return Err(AccountError::Limit);
        }
        let reference = format!(
            "{}/account/{id}/google/{}",
            self.profile,
            uuid::Uuid::new_v4()
        );
        let credentials = Credentials {
            client_id: request.registration.client_id.clone(),
            client_secret: request.registration.client_secret.clone(),
            refresh_token: token
                .refresh_token
                .as_ref()
                .ok_or(AccountError::Authorization)?
                .clone(),
        };
        self.store.prepare_credential(reference.clone()).await?;
        let saved = self
            .secret_put(
                &reference,
                Zeroizing::new(serde_json::to_vec(&credentials).map_err(|_| AccountError::Secret)?),
            )
            .await;
        if let Err(error) = saved {
            let _ = self.cleanup().await;
            return Err(error);
        }
        if let Err(error) = self
            .store
            .connect_google(ConnectedAccount {
                id: id.clone(),
                subject: info.sub,
                address: info.email,
                credential_ref: reference,
            })
            .await
        {
            let _ = self.cleanup().await;
            return Err(error.into());
        }
        *cached = Some(access(&token));
        // The new account reference is already committed. A failed deletion of an
        // obsolete secret remains durably queued; report that independently.
        let cleanup_pending = self.cleanup().await.is_err();
        Ok((id, cleanup_pending))
    }
    async fn row(&self, id: &str) -> Result<StoredAccount, AccountError> {
        validate_id(id)?;
        self.store
            .account(id.into())
            .await?
            .ok_or(AccountError::NotFound)
    }
    async fn gate(&self, id: &str) -> Arc<Mutex<Option<Access>>> {
        self.gates
            .lock()
            .await
            .entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    }
    fn validate_reference(&self, reference: &str) -> Result<(), AccountError> {
        let parts: Vec<_> = reference.split('/').collect();
        if parts.len() != 5
            || parts[0] != self.profile
            || parts[1] != "account"
            || !matches!(parts[3], "google" | "imap")
            || uuid::Uuid::parse_str(parts[2]).is_err()
            || uuid::Uuid::parse_str(parts[4]).is_err()
        {
            return Err(AccountError::Secret);
        }
        Ok(())
    }
    async fn secret_get(
        &self,
        reference: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, AccountError> {
        self.validate_reference(reference)?;
        let secrets = self.secrets.clone();
        let reference = reference.to_owned();
        let value = tokio::task::spawn_blocking(move || secrets.get(&reference))
            .await
            .map_err(|_| AccountError::Secret)?
            .map_err(|_| AccountError::Secret)?;
        if value.as_ref().is_some_and(|v| v.len() > 65536) {
            return Err(AccountError::Secret);
        }
        Ok(value)
    }
    async fn secret_put(
        &self,
        reference: &str,
        value: Zeroizing<Vec<u8>>,
    ) -> Result<(), AccountError> {
        self.validate_reference(reference)?;
        let secrets = self.secrets.clone();
        let reference = reference.to_owned();
        tokio::task::spawn_blocking(move || secrets.put(&reference, &value))
            .await
            .map_err(|_| AccountError::Secret)?
            .map_err(|_| AccountError::Secret)
    }
    async fn cleanup(&self) -> Result<(), AccountError> {
        for reference in self.store.credential_cleanup().await? {
            self.validate_reference(&reference)?;
            let secrets = self.secrets.clone();
            let name = reference.clone();
            tokio::task::spawn_blocking(move || secrets.delete(&name))
                .await
                .map_err(|_| AccountError::Secret)?
                .map_err(|_| AccountError::Secret)?;
            self.store.finish_credential_cleanup(reference).await?;
        }
        Ok(())
    }
    pub async fn shutdown(&self) {
        self.stop.send_replace(true);
        for task in self.tasks.lock().await.drain(..) {
            let _ = task.await;
        }
    }
}
fn access(response: &TokenResponse) -> Access {
    Access {
        token: Zeroizing::new(response.access_token.clone()),
        expires: Instant::now() + Duration::from_secs(response.expires_in.saturating_sub(30)),
    }
}
fn validate_id(id: &str) -> Result<(), AccountError> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| AccountError::Invalid)
}
