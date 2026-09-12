use super::{drafts, Draft, Request, Store, StoreError, WorkerOwner};
use crate::domain::mail::{BLOB_CHUNK_BYTES, MAX_PAYLOAD_BYTES};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct DraftUploadInput {
    pub account_id: String,
    pub draft_id: String,
    pub expected_version: u64,
    pub filename: Option<String>,
    pub mime_type: String,
    pub parameters: std::collections::BTreeMap<String, String>,
    pub content_id: Option<String>,
    pub disposition: String,
    pub byte_length: u64,
    pub sha256: String,
}
impl DraftUploadInput {
    pub(super) fn validate(&self) -> Result<(), StoreError> {
        crate::domain::prepare::validate_parameters(&self.parameters)
            .map_err(|_| StoreError::InvalidInput)?;
        let token = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        };
        if self.byte_length > MAX_PAYLOAD_BYTES as u64
            || self.expected_version == 0
            || self.expected_version > i64::MAX as u64
            || self.mime_type.len() > 255
            || !self
                .mime_type
                .split_once('/')
                .is_some_and(|(a, b)| token(a) && token(b))
            || self
                .filename
                .as_ref()
                .is_some_and(|f| f.is_empty() || f.len() > 1024 || f.chars().any(char::is_control))
            || self.content_id.as_ref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > 255
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_graphic() && !b"<>\"\\()[];,:".contains(&b))
            })
            || !matches!(self.disposition.as_str(), "attachment" | "inline")
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(StoreError::InvalidInput);
        }
        Ok(())
    }
}

pub struct DraftUpload {
    id: String,
    account: String,
    cleanup: Option<tokio::sync::mpsc::OwnedPermit<Request>>,
    _slot: tokio::sync::OwnedSemaphorePermit,
    _worker: Arc<WorkerOwner>,
}
impl Drop for DraftUpload {
    fn drop(&mut self) {
        if let Some(permit) = self.cleanup.take() {
            let account = self.account.clone();
            let id = self.id.clone();
            // A reserved queue slot makes cancellation cleanup synchronous to
            // enqueue, even if the async runtime or request future is stopping.
            // Startup removes staging left by process death or failed storage I/O.
            drop(permit.send(Request::Execute(Box::new(move |c| {
                let _ = c.execute(
                    "DELETE FROM draft_uploads WHERE account_id=?1 AND id=?2",
                    params![account, id],
                );
            }))));
        }
    }
}
impl Store {
    pub async fn begin_draft_upload(
        &self,
        mut input: DraftUploadInput,
        now: i64,
    ) -> Result<DraftUpload, StoreError> {
        input.validate()?;
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        input.sha256.make_ascii_lowercase();
        // Limit leases before reserving queue space: reserving all 64 slots
        // before posting writes would otherwise deadlock concurrent uploads.
        let slot = self
            .upload_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| StoreError::Busy)?;
        let cleanup = self
            .sender
            .clone()
            .reserve_owned()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        let upload = DraftUpload {
            id: uuid::Uuid::new_v4().to_string(),
            account: input.account_id.clone(),
            cleanup: Some(cleanup),
            _slot: slot,
            _worker: self._worker.clone(),
        };
        let id = upload.id.clone();
        self.execute(move|c|{
            let tx=c.transaction()?;super::accounts::writable(&tx,&input.account_id)?;let draft=drafts::get(&tx,&input.account_id,&input.draft_id)?;
            if draft.version!=input.expected_version{return Err(StoreError::VersionConflict);}
            limits(&draft,input.byte_length)?;
            let active:u32=tx.query_row("SELECT count(*) FROM draft_uploads",[],|r|r.get(0))?;
            if active>=4{return Err(StoreError::Busy);}
            let parameters=serde_json::to_string(&input.parameters).map_err(|_|StoreError::InvalidInput)?;
            tx.execute("INSERT INTO draft_uploads(account_id,id,draft_id,expected_version,filename,mime_type,content_id,disposition,byte_length,sha256,created_at_ms,parameters_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![input.account_id,id,input.draft_id,input.expected_version as i64,input.filename,input.mime_type,input.content_id,input.disposition,input.byte_length as i64,input.sha256,now,parameters])?;
            tx.commit()?;Ok(())
        }).await?;
        Ok(upload)
    }
    pub async fn append_draft_upload(
        &self,
        upload: &DraftUpload,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<(), StoreError> {
        if data.is_empty() || data.len() > BLOB_CHUNK_BYTES || offset > MAX_PAYLOAD_BYTES as u64 {
            return Err(StoreError::InvalidInput);
        }
        let account = upload.account.clone();
        let id = upload.id.clone();
        self.execute(move|c|{
            let tx=c.transaction()?;
            let (length,received):(u32,u32)=tx.query_row("SELECT byte_length,received FROM draft_uploads WHERE account_id=?1 AND id=?2",params![account,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(StoreError::NotFound)?;
            super::accounts::writable(&tx,&account)?;
            if offset!=u64::from(received) || data.len()!=(length-received).min(BLOB_CHUNK_BYTES as u32)as usize {return Err(StoreError::InvalidInput);}
            tx.execute("INSERT INTO draft_upload_chunks(account_id,upload_id,ordinal,data) VALUES (?1,?2,?3,?4)",params![account,id,received/BLOB_CHUNK_BYTES as u32,data])?;
            tx.execute("UPDATE draft_uploads SET received=received+?3 WHERE account_id=?1 AND id=?2",params![account,id,data.len()as u32])?;
            tx.commit()?;Ok(())
        }).await
    }
    pub async fn finish_draft_upload(
        &self,
        mut upload: DraftUpload,
        now: i64,
    ) -> Result<Draft, StoreError> {
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        let account = upload.account.clone();
        let id = upload.id.clone();
        let result=self.execute(move|c|{
            let tx=c.transaction()?;
            let (draft_id,version,length,received,hash):(String,i64,u32,u32,String)=tx.query_row("SELECT draft_id,expected_version,byte_length,received,sha256 FROM draft_uploads WHERE account_id=?1 AND id=?2",params![account,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?.ok_or(StoreError::NotFound)?;
            super::accounts::writable(&tx,&account)?;
            if length!=received{return Err(StoreError::InvalidInput);}
            let draft=drafts::get(&tx,&account,&draft_id)?;
            if i64::try_from(draft.version).map_err(|_|StoreError::KeyOrCorrupt)?!=version{return Err(StoreError::VersionConflict);}
            limits(&draft,u64::from(length))?;
            let mut digest=Sha256::new();let mut checked=0_u32;
            {
                let mut stmt=tx.prepare("SELECT ordinal,data FROM draft_upload_chunks WHERE account_id=?1 AND upload_id=?2 ORDER BY ordinal")?;
                let mut rows=stmt.query(params![account,id])?;
                while let Some(row)=rows.next()? {
                    let ordinal:u32=row.get(0)?;let data:Vec<u8>=row.get(1)?;
                    if checked>=length || ordinal!=checked/BLOB_CHUNK_BYTES as u32 || data.len()!=(length-checked).min(BLOB_CHUNK_BYTES as u32)as usize{return Err(StoreError::KeyOrCorrupt);}
                    digest.update(&data);checked+=data.len()as u32;
                }
            }
            if checked!=length || hex::encode(digest.finalize())!=hash{return Err(StoreError::InvalidInput);}
            let existing:Option<(String,u32)>=tx.query_row("SELECT id,byte_length FROM blobs WHERE account_id=?1 AND sha256=?2",params![account,hash],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let blob=if let Some((blob,size))=existing {
                if size!=length{return Err(StoreError::KeyOrCorrupt);}blob
            }else{
                let blob=uuid::Uuid::new_v4().to_string();
                tx.execute("INSERT INTO blobs(account_id,id,sha256,byte_length) VALUES (?1,?2,?3,?4)",params![account,blob,hash,length])?;
                tx.execute("INSERT INTO blob_chunks(account_id,blob_id,ordinal,data) SELECT account_id,?3,ordinal,data FROM draft_upload_chunks WHERE account_id=?1 AND upload_id=?2",params![account,id,blob])?;
                blob
            };
            let attachment=uuid::Uuid::new_v4().to_string();
            tx.execute("INSERT INTO draft_attachments(account_id,draft_id,id,position,blob_id,filename,mime_type,content_id,disposition,parameters_json) SELECT account_id,draft_id,?3,?4,?5,filename,mime_type,content_id,disposition,parameters_json FROM draft_uploads WHERE account_id=?1 AND id=?2",params![account,id,attachment,draft.attachments.len()as u32,blob])?;
            tx.execute("UPDATE drafts SET version=version+1,updated_at_ms=?3 WHERE account_id=?1 AND id=?2",params![account,draft_id,now.max(draft.updated_at_ms)])?;
            tx.execute("DELETE FROM draft_uploads WHERE account_id=?1 AND id=?2",params![account,id])?;
            super::changes::record(&tx,Some(&account),"draft",Some(&draft_id))?;
            let draft=drafts::get(&tx,&account,&draft_id)?;tx.commit()?;Ok(draft)
        }).await?;
        upload.cleanup.take();
        Ok(result)
    }
    pub async fn recover_draft_uploads(&self) -> Result<u64, StoreError> {
        self.execute(|c| Ok(c.execute("DELETE FROM draft_uploads", [])? as u64))
            .await
    }
}
fn limits(draft: &Draft, length: u64) -> Result<(), StoreError> {
    if draft.attachments.len() >= 256
        || draft
            .attachments
            .iter()
            .fold(length, |n, a| n.saturating_add(a.byte_length))
            > MAX_PAYLOAD_BYTES as u64
    {
        return Err(StoreError::ResultTooLarge);
    }
    Ok(())
}
