use super::*;
impl Store {
    pub(crate) async fn stage_cached_imap_mail(
        &self,
        run: String,
        placement: ImapPlacement,
        flags: ImapFlags,
    ) -> Result<bool, StoreError> {
        let account = placement.account_id.to_string();
        let provider = placement
            .provider_id()
            .map_err(|_| StoreError::InvalidInput)?;
        let json = serde_json::to_string(&ImapMessageState { placement, flags })
            .map_err(|_| StoreError::InvalidInput)?;
        self.execute(move|c| {
            let tx=c.transaction()?;check_run(&tx,&account,&run)?;validate_message(&tx,&account,&run,&provider,&json)?;
            let id:Option<String>=tx.query_row("SELECT id FROM messages WHERE account_id=?1 AND provider_id=?2 AND body_availability<>'missing'",params![account,provider],|r|r.get(0)).optional()?;
            let Some(id)=id else {return Ok(false);};
            tx.execute("DELETE FROM staged_messages WHERE account_id=?1 AND run_id=?2 AND provider_id=?3",params![account,run,provider])?;
            tx.execute("INSERT INTO staged_messages(account_id,run_id,provider_id,thread_id,history_id,internal_date_ms,subject,provider_json,raw_blob_id,text_blob_id,html_blob_id,search_text,body_availability) SELECT m.account_id,?2,m.provider_id,m.thread_id,m.history_id,m.internal_date_ms,m.subject,?4,m.raw_blob_id,m.text_blob_id,m.html_blob_id,s.body,m.body_availability FROM messages m JOIN message_search s ON s.account_id=m.account_id AND s.message_id=m.id WHERE m.account_id=?1 AND m.provider_id=?3",params![account,run,provider,json])?;
            tx.execute("INSERT INTO staged_headers SELECT account_id,?2,?3,ordinal,name,value FROM message_headers WHERE account_id=?1 AND message_id=?4",params![account,run,provider,id])?;
            tx.execute("INSERT INTO staged_memberships VALUES(?1,?2,?3,?4)",params![account,run,provider,placement.mailbox_id.to_string()])?;
            tx.execute("INSERT INTO staged_attachments SELECT account_id,?2,?3,part_index,filename,mime_type,content_id,blob_id FROM attachments WHERE account_id=?1 AND message_id=?4",params![account,run,provider,id])?;
            tx.commit()?;Ok(true)
        }).await
    }
    pub(crate) async fn stage_absent_imap_mail(
        &self,
        account: String,
        run: String,
        mailbox: ImapMailboxId,
        epoch: u32,
        uids: Vec<u32>,
    ) -> Result<(), StoreError> {
        if epoch == 0 || uids.len() > 1_000_000 || uids.contains(&0) {
            return Err(StoreError::InvalidInput);
        }
        let uids = serde_json::to_string(&uids).map_err(|_| StoreError::InvalidInput)?;
        self.execute(move|c| {
            let tx=c.transaction()?;check_run(&tx,&account,&run)?;
            let staged:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM staged_imap_mailboxes WHERE account_id=?1 AND run_id=?2 AND id=?3 AND json_extract(state_json,'$.uid_validity')=?4)",params![account,run,mailbox.to_string(),epoch],|r|r.get(0))?;
            if !staged {return Err(StoreError::InvalidInput);}
            tx.execute("INSERT INTO staged_messages(account_id,run_id,provider_id,body_availability,deleted) SELECT m.account_id,?2,m.provider_id,'missing',1 FROM messages m JOIN imap_placements p ON p.account_id=m.account_id AND p.message_id=m.id WHERE p.account_id=?1 AND p.mailbox_id=?3 AND p.uid_validity=?4 AND p.uid NOT IN (SELECT value FROM json_each(?5)) ON CONFLICT(account_id,run_id,provider_id) DO UPDATE SET deleted=1",params![account,run,mailbox.to_string(),epoch,uids])?;
            tx.commit()?;Ok(())
        }).await
    }
}
