use super::{Account, AccountError, Accounts};
use crate::{
    domain::imap_account::{ImapAccountConfig, ImapCapabilities, ImapCredentials},
    providers::imap::probe,
    store::{ConnectedAccount, StoredAccount},
};
use zeroize::Zeroizing;

pub struct ImapConnection {
    pub account: Account,
    pub capabilities: ImapCapabilities,
    pub credential_cleanup_pending: bool,
}
impl Accounts {
    pub(crate) async fn imap_session(
        &self,
        id: &str,
    ) -> Result<crate::providers::imap::Connection, AccountError> {
        let gate = self.gate(id).await;
        let _guard = gate.lock().await;
        let row = self.row(id).await?;
        if row.account.provider != "imap" {
            return Err(AccountError::Invalid);
        }
        if row.state != "connected" {
            return Err(AccountError::Authorization);
        }
        let secret = self
            .secret_get(
                row.credential_ref
                    .as_deref()
                    .ok_or(AccountError::Authorization)?,
            )
            .await?
            .ok_or(AccountError::Authorization)?;
        let credentials: ImapCredentials =
            serde_json::from_slice(&secret).map_err(|_| AccountError::Secret)?;
        credentials.validate().map_err(|_| AccountError::Secret)?;
        let (config, previous) = self.store.imap_config(id.into()).await?;
        if config
            .identity()
            .map_err(|_| AccountError::Storage)?
            .as_str()
            != row.subject.as_deref().ok_or(AccountError::Storage)?
        {
            return Err(AccountError::IdentityMismatch);
        }
        match crate::providers::imap::open(&config, &credentials)
            .await
            .map_err(AccountError::from)
        {
            Ok(connection) => {
                let mut capabilities = connection.capabilities.clone();
                capabilities.smtp_utf8 = previous.smtp_utf8;
                capabilities.eight_bit_mime = previous.eight_bit_mime;
                self.store
                    .update_imap_capabilities(id.into(), capabilities)
                    .await?;
                Ok(connection)
            }
            Err(AccountError::Authorization) => {
                self.store
                    .set_account_state(id.into(), "needs_auth".into())
                    .await?;
                self.cleanup().await?;
                Err(AccountError::Authorization)
            }
            Err(error) => Err(error),
        }
    }
    pub(crate) async fn smtp_session(
        &self,
        id: &str,
        expected: &crate::domain::imap_account::MailEndpoint,
    ) -> Result<crate::providers::imap::smtp::Session, AccountError> {
        let gate = self.gate(id).await;
        let _guard = gate.lock().await;
        let row = self.row(id).await?;
        if row.account.provider != "imap" {
            return Err(AccountError::Invalid);
        }
        if row.state != "connected" {
            return Err(AccountError::Authorization);
        }
        let (config, _) = self.store.imap_config(id.into()).await?;
        if &config.smtp != expected
            || config
                .identity()
                .map_err(|_| AccountError::Storage)?
                .as_str()
                != row.subject.as_deref().ok_or(AccountError::Storage)?
        {
            return Err(AccountError::IdentityMismatch);
        }
        let secret = self
            .secret_get(
                row.credential_ref
                    .as_deref()
                    .ok_or(AccountError::Authorization)?,
            )
            .await?
            .ok_or(AccountError::Authorization)?;
        let credentials: ImapCredentials =
            serde_json::from_slice(&secret).map_err(|_| AccountError::Secret)?;
        credentials.validate().map_err(|_| AccountError::Secret)?;
        let result = crate::providers::imap::open_smtp(&config, &credentials)
            .await
            .map_err(AccountError::from);
        if matches!(result, Err(AccountError::Authorization)) {
            self.store
                .set_account_state(id.into(), "needs_auth".into())
                .await?;
            self.cleanup().await?;
        }
        result
    }
    pub async fn imap_config(
        &self,
        id: String,
    ) -> Result<(ImapAccountConfig, ImapCapabilities), AccountError> {
        let row = self.row(&id).await?;
        if row.account.provider != "imap" {
            return Err(AccountError::Invalid);
        }
        Ok(self.store.imap_config(id).await?)
    }
    pub async fn connect_imap(
        &self,
        config: ImapAccountConfig,
        credentials: ImapCredentials,
        requested: Option<String>,
    ) -> Result<ImapConnection, AccountError> {
        credentials.validate().map_err(|_| AccountError::Invalid)?;
        let identity = config.identity().map_err(|_| AccountError::Invalid)?;
        let id = if let Some(id) = requested {
            let row = self.row(&id).await?;
            if row.account.provider != "imap" || row.subject.as_deref() != Some(&identity) {
                return Err(AccountError::IdentityMismatch);
            }
            id
        } else {
            let mut found = None;
            for account in self.store.accounts().await? {
                let row = self.row(&account.id).await?;
                if row.account.provider == "imap" && row.subject.as_deref() == Some(&identity) {
                    found = Some(account.id);
                    break;
                }
            }
            found.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        };
        let gate = self.gate(&id).await;
        let _credentials = gate.lock().await;
        let _mutation = self.mutations.lock().await;
        if let Some(row) = self.store.account(id.clone()).await? {
            if row.account.provider != "imap" || row.subject.as_deref() != Some(&identity) {
                return Err(AccountError::IdentityMismatch);
            }
        } else if self.store.status().await?.account_count >= 100 {
            return Err(AccountError::Limit);
        }
        let mut stop = self.stop.subscribe();
        if *stop.borrow() {
            return Err(AccountError::Unavailable);
        }
        let capabilities = tokio::select! {
            _=stop.changed()=>return Err(AccountError::Unavailable),
            result=probe(&config,&credentials)=>result.map_err(AccountError::from)?,
        };
        let reference = format!(
            "{}/account/{id}/imap/{}",
            self.profile,
            uuid::Uuid::new_v4()
        );
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
            .connect_imap(
                ConnectedAccount {
                    id: id.clone(),
                    subject: identity,
                    address: config.address.clone(),
                    credential_ref: reference,
                },
                config,
                capabilities.clone(),
            )
            .await
        {
            let _ = self.cleanup().await;
            return Err(error.into());
        }
        let credential_cleanup_pending = self.cleanup().await.is_err();
        Ok(ImapConnection {
            account: self.row(&id).await?.into(),
            capabilities,
            credential_cleanup_pending,
        })
    }
    pub(super) async fn check_imap(&self, row: StoredAccount) -> Result<Account, AccountError> {
        if row.state != "connected" {
            return Err(AccountError::Authorization);
        }
        let reference = row
            .credential_ref
            .as_ref()
            .ok_or(AccountError::Authorization)?;
        let secret = self
            .secret_get(reference)
            .await?
            .ok_or(AccountError::Authorization)?;
        let credentials: ImapCredentials =
            serde_json::from_slice(&secret).map_err(|_| AccountError::Secret)?;
        credentials.validate().map_err(|_| AccountError::Secret)?;
        let (config, _) = self.store.imap_config(row.account.id.clone()).await?;
        if config
            .identity()
            .map_err(|_| AccountError::Storage)?
            .as_str()
            != row.subject.as_deref().ok_or(AccountError::Storage)?
        {
            return Err(AccountError::IdentityMismatch);
        }
        let mut stop = self.stop.subscribe();
        if *stop.borrow() {
            return Err(AccountError::Unavailable);
        }
        let result = tokio::select! {
            _=stop.changed()=>return Err(AccountError::Unavailable),
            result=probe(&config,&credentials)=>result.map_err(AccountError::from),
        };
        match result {
            Ok(caps) => {
                self.store
                    .update_imap_capabilities(row.account.id.clone(), caps)
                    .await?;
                Ok(row.into())
            }
            Err(AccountError::Authorization) => {
                self.store
                    .set_account_state(row.account.id, "needs_auth".into())
                    .await?;
                self.cleanup().await?;
                Err(AccountError::Authorization)
            }
            Err(error) => Err(error),
        }
    }
}
