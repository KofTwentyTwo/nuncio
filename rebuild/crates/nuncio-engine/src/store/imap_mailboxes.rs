use super::{StagedMail, Store, StoreError};
use crate::domain::{
    identity::{AccountId, ImapMailboxId},
    imap::{ImapFlags, ImapMailboxState, ImapMessageState, ImapPlacement},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
mod cached;
mod promotion;
pub(super) use promotion::{finish, prepare};

#[derive(Clone, Serialize)]
pub struct StoredImapMailbox {
    pub id: ImapMailboxId,
    pub state: ImapMailboxState,
    pub retired: bool,
}
impl Store {
    pub async fn stage_imap_mailbox(
        &self,
        account: String,
        run: String,
        state: ImapMailboxState,
    ) -> Result<ImapMailboxId, StoreError> {
        let state = state.validated().map_err(|_| StoreError::InvalidInput)?;
        self.execute(move|c| {
            let tx=c.transaction()?;check_run(&tx,&account,&run)?;
            let id:Option<String>=tx.query_row("SELECT id FROM staged_imap_mailboxes WHERE account_id=?1 AND run_id=?2 AND name=?3",params![account,run,state.name.as_str()],|r|r.get(0)).optional()?;
            let id=match id {
                Some(id)=>id,
                None=>tx.query_row("SELECT id FROM imap_mailboxes WHERE account_id=?1 AND name=?2 AND retired=0",params![account,state.name.as_str()],|r|r.get(0)).optional()?.unwrap_or_else(||ImapMailboxId::generate().to_string()),
            };
            let count:u32=tx.query_row("SELECT count(*) FROM staged_imap_mailboxes WHERE account_id=?1 AND run_id=?2",params![account,run],|r|r.get(0))?;
            if count>=4096 {return Err(StoreError::ResultTooLarge);}
            tx.execute("INSERT INTO staged_imap_mailboxes(account_id,run_id,id,name,state_json) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,run_id,id) DO UPDATE SET state_json=excluded.state_json",params![account,run,id,state.name.as_str(),serde_json::to_string(&state).map_err(|_|StoreError::InvalidInput)?])?;
            tx.execute("INSERT INTO staged_collections(account_id,run_id,provider_id,name,kind) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,run_id,provider_id) DO UPDATE SET name=excluded.name,kind=excluded.kind",params![account,run,id,state.name.as_str(),kind(&state)])?;
            tx.commit()?;id.parse().map_err(|_|StoreError::KeyOrCorrupt)
        }).await
    }
    pub async fn stage_imap_mail(
        &self,
        run: String,
        placement: ImapPlacement,
        flags: ImapFlags,
        mut mail: StagedMail,
    ) -> Result<(), StoreError> {
        let account = placement.account_id.to_string();
        if mail.provider_id
            != placement
                .provider_id()
                .map_err(|_| StoreError::InvalidInput)?
        {
            return Err(StoreError::InvalidInput);
        }
        let data = ImapMessageState { placement, flags };
        let encoded = serde_json::to_string(&data).map_err(|_| StoreError::InvalidInput)?;
        let check_account = account.clone();
        let check_run_id = run.clone();
        let check_json = encoded.clone();
        let check_provider = mail.provider_id.clone();
        self.execute(move |c| {
            check_run(c, &check_account, &check_run_id)?;
            validate_message(
                c,
                &check_account,
                &check_run_id,
                &check_provider,
                &check_json,
            )?;
            Ok(())
        })
        .await?;
        mail.provider_json = encoded;
        mail.labels = vec![placement.mailbox_id.to_string()];
        // Incomplete staged data is never published; promotion repeats all
        // placement/epoch checks inside its publication transaction.
        self.stage_mail(account, run, mail).await
    }
    pub async fn imap_mailboxes(
        &self,
        account: String,
    ) -> Result<Vec<StoredImapMailbox>, StoreError> {
        self.execute(move|c| {
            let mut stmt=c.prepare("SELECT id,state_json,retired FROM imap_mailboxes WHERE account_id=?1 ORDER BY name,id LIMIT 4097")?;
            let mut rows=stmt.query([account])?;let mut result=Vec::new();
            while let Some(row)=rows.next()? {
                if result.len()>=4096 {return Err(StoreError::ResultTooLarge);}
                let id:String=row.get(0)?;let state:String=row.get(1)?;
                result.push(StoredImapMailbox{id:id.parse().map_err(|_|StoreError::KeyOrCorrupt)?,state:serde_json::from_str(&state).map_err(|_|StoreError::KeyOrCorrupt)?,retired:row.get(2)?});
            }
            Ok(result)
        }).await
    }
}
fn check_run(c: &Connection, account: &str, run: &str) -> Result<(), StoreError> {
    super::mail_staging::running(c, account, run)?;
    if super::sync::get(c, account, run)?.scope != "imap" {
        return Err(StoreError::InvalidInput);
    }
    let provider: String = c.query_row(
        "SELECT provider FROM accounts WHERE id=?1",
        [account],
        |r| r.get(0),
    )?;
    if provider != "imap" {
        return Err(StoreError::InvalidAccount);
    }
    Ok(())
}
fn kind(state: &ImapMailboxState) -> &'static str {
    if state.name.as_str() == "INBOX"
        || state.attributes.iter().any(|a| {
            ["\\Sent", "\\Archive", "\\Trash", "\\Junk", "\\Drafts"]
                .iter()
                .any(|role| a.eq_ignore_ascii_case(role))
        })
    {
        "system"
    } else {
        "user"
    }
}
fn validate_message(
    c: &Connection,
    account: &str,
    run: &str,
    provider: &str,
    json: &str,
) -> Result<ImapMessageState, StoreError> {
    let data: ImapMessageState =
        serde_json::from_str(json).map_err(|_| StoreError::InvalidInput)?;
    if data.placement.account_id
        != account
            .parse::<AccountId>()
            .map_err(|_| StoreError::InvalidAccount)?
        || data
            .placement
            .provider_id()
            .map_err(|_| StoreError::InvalidInput)?
            != provider
    {
        return Err(StoreError::InvalidInput);
    }
    let state:String=c.query_row("SELECT state_json FROM staged_imap_mailboxes WHERE account_id=?1 AND run_id=?2 AND id=?3",params![account,run,data.placement.mailbox_id.to_string()],|r|r.get(0)).optional()?.ok_or(StoreError::InvalidInput)?;
    let state: ImapMailboxState =
        serde_json::from_str(&state).map_err(|_| StoreError::KeyOrCorrupt)?;
    if state.uid_validity != Some(data.placement.uid_validity.get())
        || state
            .uid_next
            .is_none_or(|next| data.placement.uid.get() >= next)
    {
        return Err(StoreError::InvalidInput);
    }
    Ok(data)
}
