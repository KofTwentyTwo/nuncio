use super::{blobs, sync, StagedMail, Store, StoreError};
use rusqlite::{params, Connection};

pub(super) fn running(connection: &Connection, account: &str, run: &str) -> Result<(), StoreError> {
    if sync::get(connection, account, run)?.state != "running" {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}
impl Store {
    pub async fn stage_mail_collection(
        &self,
        account: String,
        run: String,
        provider_id: String,
        name: Option<String>,
        kind: String,
    ) -> Result<(), StoreError> {
        if provider_id.is_empty()
            || provider_id.len() > 2048
            || name.as_ref().is_some_and(|n| n.len() > 8192)
            || !matches!(kind.as_str(), "system" | "user" | "unknown")
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            running(c,&account,&run)?;
            c.execute("INSERT INTO staged_collections(account_id,run_id,provider_id,name,kind) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(account_id,run_id,provider_id) DO UPDATE SET name=excluded.name,kind=excluded.kind",params![account,run,provider_id,name,kind])?;Ok(())
        }).await
    }
    pub async fn stage_mail(
        &self,
        account: String,
        run: String,
        mut mail: StagedMail,
    ) -> Result<(), StoreError> {
        if mail.provider_id.is_empty()
            || mail.provider_id.len() > 2048
            || mail.labels.len() > 4096
            || mail.labels.iter().any(|l| l.is_empty() || l.len() > 2048)
            || mail.provider_json.len() > 256 * 1024
            || !matches!(
                mail.availability.as_str(),
                "available" | "missing" | "unparsed" | "too_large"
            )
            || (mail.availability == "available" && (mail.decoded.is_none() || mail.raw.is_none()))
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;running(&tx,&account,&run)?;
            tx.execute("DELETE FROM staged_messages WHERE account_id=?1 AND run_id=?2 AND provider_id=?3",params![account,run,mail.provider_id])?;
            let raw=mail.raw.as_ref().map(|b|blobs::insert(&tx,&account,b)).transpose()?;
            let text=mail.decoded.as_ref().and_then(|d|d.text.as_ref()).map(|b|blobs::insert(&tx,&account,b.as_bytes())).transpose()?;
            let html=mail.decoded.as_ref().and_then(|d|d.html.as_ref()).map(|b|blobs::insert(&tx,&account,b.as_bytes())).transpose()?;
            if let Some(decoded)=mail.decoded.as_ref() {mail.subject=decoded.subject.clone();mail.headers=decoded.headers.clone();}
            tx.execute("INSERT INTO staged_messages(account_id,run_id,provider_id,thread_id,history_id,internal_date_ms,subject,provider_json,raw_blob_id,text_blob_id,html_blob_id,search_text,body_availability) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![account,run,mail.provider_id,mail.thread_id,mail.history_id,mail.internal_date_ms,mail.subject,mail.provider_json,raw.map(|b|b.id),text.map(|b|b.id),html.map(|b|b.id),mail.decoded.as_ref().map(|d|&d.search_text),mail.availability])?;
            for (ordinal,h) in mail.headers.iter().enumerate(){
                let ordinal=i64::try_from(ordinal).map_err(|_|StoreError::InvalidInput)?;
                tx.execute("INSERT INTO staged_headers(account_id,run_id,provider_id,ordinal,name,value) VALUES (?1,?2,?3,?4,?5,?6)",params![account,run,mail.provider_id,ordinal,h.name,h.value])?;
            }
            for label in &mail.labels {
                tx.execute("INSERT OR IGNORE INTO staged_memberships(account_id,run_id,provider_id,label_id) VALUES (?1,?2,?3,?4)",params![account,run,mail.provider_id,label])?;
            }
            if let Some(decoded)=mail.decoded {
                for attachment in decoded.attachments {
                    let blob=blobs::insert(&tx,&account,&attachment.bytes)?;
                    tx.execute("INSERT INTO staged_attachments(account_id,run_id,provider_id,part_index,filename,mime_type,content_id,blob_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![account,run,mail.provider_id,attachment.part_index,attachment.filename,attachment.mime_type,attachment.content_id,blob.id])?;
                }
            }
            tx.commit()?;Ok(())
        }).await
    }
    pub async fn stage_mail_deletion(
        &self,
        account: String,
        run: String,
        provider_id: String,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;running(&tx,&account,&run)?;
            tx.execute("DELETE FROM staged_messages WHERE account_id=?1 AND run_id=?2 AND provider_id=?3",params![account,run,provider_id])?;
            tx.execute("INSERT INTO staged_messages(account_id,run_id,provider_id,body_availability,deleted) VALUES (?1,?2,?3,'missing',1)",params![account,run,provider_id])?;
            tx.commit()?;Ok(())
        }).await
    }
}
