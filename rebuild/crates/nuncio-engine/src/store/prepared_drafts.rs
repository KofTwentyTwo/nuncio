use super::{Draft, DraftUploadInput, Store, StoreError, StoredBlob};
use crate::domain::{
    mail::{BLOB_CHUNK_BYTES, MAX_PAYLOAD_BYTES},
    prepare::PreparedDraft,
};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

pub(crate) struct DraftSource {
    pub address: String,
    pub thread_id: Option<String>,
    pub raw: StoredBlob,
}
impl Store {
    pub(crate) async fn draft_source(
        &self,
        account: String,
        message: String,
    ) -> Result<DraftSource, StoreError> {
        self.execute(move|c| c.query_row("SELECT a.address,m.thread_id,b.id,b.byte_length,b.sha256 FROM messages m JOIN accounts a ON a.id=m.account_id JOIN blobs b ON b.account_id=m.account_id AND b.id=m.raw_blob_id WHERE m.account_id=?1 AND m.id=?2",params![account,message],|r|Ok(DraftSource {address:r.get(0)?,thread_id:r.get(1)?,raw:StoredBlob{id:r.get(2)?,byte_length:u64::from(r.get::<_,u32>(3)?),sha256:r.get(4)?}})).optional()?.ok_or(StoreError::NotFound)).await
    }
    pub(crate) async fn read_blob(
        &self,
        account: String,
        blob: StoredBlob,
    ) -> Result<Vec<u8>, StoreError> {
        if blob.byte_length > MAX_PAYLOAD_BYTES as u64 {
            return Err(StoreError::ResultTooLarge);
        }
        let mut bytes = Vec::with_capacity(blob.byte_length as usize);
        let mut ordinal = 0;
        while bytes.len() < blob.byte_length as usize {
            let chunk = self
                .blob_chunk(account.clone(), blob.id.clone(), ordinal)
                .await?;
            if chunk.len() != (blob.byte_length as usize - bytes.len()).min(BLOB_CHUNK_BYTES) {
                return Err(StoreError::KeyOrCorrupt);
            }
            bytes.extend(chunk);
            ordinal += 1;
        }
        if hex::encode(Sha256::digest(&bytes)) != blob.sha256 {
            return Err(StoreError::KeyOrCorrupt);
        }
        Ok(bytes)
    }
    pub(crate) async fn create_prepared_draft(
        &self,
        account: String,
        prepared: PreparedDraft,
        now: i64,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<Draft, StoreError> {
        prepared
            .content
            .validate()
            .map_err(|_| StoreError::InvalidInput)?;
        prepared
            .context
            .validate()
            .map_err(|_| StoreError::InvalidInput)?;
        if now < 0
            || prepared.attachments.len() > 256
            || prepared
                .attachments
                .iter()
                .map(|a| a.bytes.len())
                .sum::<usize>()
                > MAX_PAYLOAD_BYTES
        {
            return Err(StoreError::ResultTooLarge);
        }
        for attachment in &prepared.attachments {
            DraftUploadInput {
                account_id: account.clone(),
                draft_id: String::new(),
                expected_version: 1,
                filename: attachment.filename.clone(),
                mime_type: attachment.mime_type.clone(),
                parameters: attachment.parameters.clone(),
                content_id: attachment.content_id.clone(),
                disposition: attachment.disposition.clone(),
                byte_length: attachment.bytes.len() as u64,
                sha256: "0".repeat(64),
            }
            .validate()?;
        }
        self.execute(move|c|{
            // The memory admission permit remains owned through the queued write,
            // including when the requesting RPC is cancelled.
            let _permit=permit;
            let tx=c.transaction()?;
            let id=uuid::Uuid::new_v4().to_string();
            let content=serde_json::to_string(&prepared.content).map_err(|_|StoreError::InvalidInput)?;
            let context=serde_json::to_string(&prepared.context).map_err(|_|StoreError::InvalidInput)?;
            super::accounts::writable(&tx,&account)?;
            tx.execute("INSERT INTO drafts(account_id,id,version,subject,content_json,created_at_ms,updated_at_ms,context_json) VALUES (?1,?2,1,?3,?4,?5,?5,?6)",params![account,id,prepared.content.subject,content,now,context])?;
            for (position,a) in prepared.attachments.into_iter().enumerate() {
                let blob=super::blobs::insert(&tx,&account,&a.bytes)?;
                tx.execute("INSERT INTO draft_attachments(account_id,draft_id,id,position,blob_id,filename,mime_type,parameters_json,content_id,disposition) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![account,id,uuid::Uuid::new_v4().to_string(),position as u32,blob.id,a.filename,a.mime_type,serde_json::to_string(&a.parameters).map_err(|_|StoreError::InvalidInput)?,a.content_id,a.disposition])?;
            }
            super::changes::record(&tx,Some(&account),"draft",Some(&id))?;
            let draft=super::drafts::get(&tx,&account,&id)?;tx.commit()?;Ok(draft)
        }).await
    }
}
