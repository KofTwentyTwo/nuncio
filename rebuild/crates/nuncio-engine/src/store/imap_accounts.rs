use super::{ConnectedAccount, Store, StoreError};
use crate::domain::imap_account::{ImapAccountConfig, ImapCapabilities};
use rusqlite::{params, OptionalExtension};
impl Store {
    pub async fn connect_imap(
        &self,
        account: ConnectedAccount,
        config: ImapAccountConfig,
        capabilities: ImapCapabilities,
    ) -> Result<(), StoreError> {
        let config = config
            .canonicalized()
            .map_err(|_| StoreError::InvalidAccount)?;
        if config.address != account.address
            || config.identity().map_err(|_| StoreError::InvalidAccount)? != account.subject
        {
            return Err(StoreError::InvalidAccount);
        }
        self.execute(move |c| {
            super::accounts::connect_provider(c, account, Some((config, capabilities)))
        })
        .await
    }
    pub async fn imap_config(
        &self,
        id: String,
    ) -> Result<(ImapAccountConfig, ImapCapabilities), StoreError> {
        self.execute(move |c| {
            let (config, caps): (String, String) = c
                .query_row(
                    "SELECT config,capabilities FROM imap_accounts WHERE account_id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .ok_or(StoreError::NotFound)?;
            Ok((
                serde_json::from_str(&config).map_err(|_| StoreError::KeyOrCorrupt)?,
                serde_json::from_str(&caps).map_err(|_| StoreError::KeyOrCorrupt)?,
            ))
        })
        .await
    }
    pub async fn update_imap_capabilities(
        &self,
        id: String,
        caps: ImapCapabilities,
    ) -> Result<(), StoreError> {
        self.execute(move |c| {
            let tx = c.transaction()?;
            let value = serde_json::to_string(&caps).map_err(|_| StoreError::InvalidInput)?;
            if tx.execute(
                "UPDATE imap_accounts SET capabilities=?2 WHERE account_id=?1 AND capabilities<>?2",
                params![id, value],
            )? > 0
            {
                super::changes::record(&tx, Some(&id), "account_capabilities", None)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
