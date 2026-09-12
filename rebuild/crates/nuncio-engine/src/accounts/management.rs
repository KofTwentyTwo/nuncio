use super::{Account, AccountError, Accounts, AuthSession};
use crate::store::{AccountLifecycle, AccountPurgePreview};

impl Accounts {
    pub async fn show(&self, id: &str) -> Result<Account, AccountError> {
        Ok(self.row(id).await?.into())
    }

    pub async fn edit_name(
        &self,
        id: &str,
        version: u64,
        name: String,
    ) -> Result<Account, AccountError> {
        self.row(id).await?;
        let gate = self.gate(id).await;
        let _access = gate.lock().await;
        let _mutation = self.mutations.lock().await;
        Ok(self
            .store
            .edit_account_name(id.into(), version, name)
            .await?
            .into())
    }

    pub async fn lifecycle(
        &self,
        id: &str,
        action: AccountLifecycle,
    ) -> Result<(Account, bool), AccountError> {
        self.row(id).await?;
        let gate = self.gate(id).await;
        let mut access = gate.lock().await;
        let _mutation = self.mutations.lock().await;
        #[cfg(feature = "test-harness")]
        if matches!(action, AccountLifecycle::Archive) {
            self.management_checkpoint("account_before_archive").await?;
        }
        let row = self
            .store
            .change_account_lifecycle(id.into(), action)
            .await?;
        let pending = if matches!(action, AccountLifecycle::Archive) {
            #[cfg(feature = "test-harness")]
            self.management_checkpoint("account_after_archive").await?;
            self.cancel_account_sessions(id).await;
            *access = None;
            self.cleanup().await.is_err()
        } else {
            false
        };
        Ok((row.into(), pending))
    }

    pub async fn purge(
        &self,
        id: &str,
        version: u64,
        revision: u64,
    ) -> Result<(AccountPurgePreview, bool), AccountError> {
        self.row(id).await?;
        let gate = self.gate(id).await;
        let mut access = gate.lock().await;
        let _mutation = self.mutations.lock().await;
        #[cfg(feature = "test-harness")]
        self.management_checkpoint("account_before_purge").await?;
        let preview = self
            .store
            .purge_account(id.into(), version, revision)
            .await?;
        #[cfg(feature = "test-harness")]
        self.management_checkpoint("account_after_purge").await?;
        self.cancel_account_sessions(id).await;
        *access = None;
        let pending = self.cleanup().await.is_err();
        Ok((preview, pending))
    }

    #[cfg(feature = "test-harness")]
    async fn management_checkpoint(&self, name: &str) -> Result<(), AccountError> {
        if let Some(config) = &self.http.test_config {
            config
                .checkpoint(name, self.stop.subscribe())
                .await
                .map_err(|_| AccountError::Unavailable)?;
        }
        Ok(())
    }

    async fn cancel_account_sessions(&self, id: &str) {
        for slot in self.sessions.lock().await.values_mut() {
            // An untargeted consent flow has no known identity until its callback finishes.
            if (slot.expected_account.is_none() || slot.expected_account.as_deref() == Some(id))
                && matches!(slot.status.state.as_str(), "pending" | "completing")
            {
                slot.status.state = "cancelled".into();
                slot.status.browser_url.clear();
                slot.cancel.send_replace(true);
            }
        }
    }

    pub async fn cancel_auth(&self, id: &str) -> Result<AuthSession, AccountError> {
        super::validate_id(id)?;
        let _mutation = self.mutations.lock().await;
        let mut sessions = self.sessions.lock().await;
        let slot = sessions.get_mut(id).ok_or(AccountError::NotFound)?;
        if matches!(slot.status.state.as_str(), "pending" | "completing") {
            slot.status.state = "cancelled".into();
            slot.status.browser_url.clear();
            slot.cancel.send_replace(true);
        }
        Ok(slot.status.clone())
    }
}
