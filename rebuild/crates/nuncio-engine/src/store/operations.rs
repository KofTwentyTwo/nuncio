use super::{Store, StoreError, StoredBlob};
use crate::domain::{drafts::Recipient, submission::FrozenMessage};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct EnqueueSend {
    pub account_id: String,
    pub request_id: String,
    pub draft_id: String,
    /// Part of the client's request identity. None means snapshot at first enqueue.
    pub expected_version: Option<u64>,
    /// Always checked on the first insert after MIME was frozen outside SQLite.
    pub snapshot_version: u64,
    pub sender: String,
    pub frozen: FrozenMessage,
}
#[derive(Clone, Serialize)]
pub struct Operation {
    pub id: String,
    pub account_id: String,
    pub request_id: String,
    pub kind: String,
    pub resource_id: String,
    pub fingerprint: String,
    pub state: String,
    pub version: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub next_attempt_at_ms: Option<i64>,
    pub needs_reconciliation: bool,
    pub error_code: Option<String>,
    pub disposition: Option<String>,
    pub desired_state: Value,
    pub resolutions: Vec<super::OperationResolution>,
    pub reconciliation: Option<super::ReconciliationRequest>,
}
pub struct SendPayload {
    pub draft_id: String,
    pub draft_version: u64,
    pub sender: String,
    pub recipients: Vec<String>,
    pub message_id: String,
    pub thread_id: Option<String>,
    pub wire: StoredBlob,
    pub sent_copy: Option<StoredBlob>,
}
impl Store {
    pub async fn find_send_request(
        &self,
        account: String,
        request_id: String,
        draft_id: String,
        expected_version: Option<u64>,
    ) -> Result<Option<Operation>, StoreError> {
        let request_id = uuid::Uuid::parse_str(&request_id)
            .map_err(|_| StoreError::InvalidInput)?
            .to_string();
        let (_, fingerprint) = send_identity(&draft_id, expected_version)?;
        self.execute(move |c| {
            let id: Option<String> = c
                .query_row(
                    "SELECT id FROM operations WHERE account_id=?1 AND request_id=?2",
                    params![account, request_id],
                    |r| r.get(0),
                )
                .optional()?;
            id.map(|id| {
                let op = get(c, &account, &id)?;
                if op.fingerprint != fingerprint {
                    return Err(StoreError::VersionConflict);
                }
                Ok(op)
            })
            .transpose()
        })
        .await
    }
    pub async fn enqueue_send(
        &self,
        input: EnqueueSend,
        now: i64,
    ) -> Result<Operation, StoreError> {
        self.enqueue_send_guarded(input, now, None).await
    }
    pub(crate) async fn enqueue_send_guarded(
        &self,
        input: EnqueueSend,
        now: i64,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<Operation, StoreError> {
        let request_id = uuid::Uuid::parse_str(&input.request_id)
            .map_err(|_| StoreError::InvalidInput)?
            .to_string();
        if now < 0
            || input.snapshot_version == 0
            || input.snapshot_version > i64::MAX as u64
            || input
                .expected_version
                .is_some_and(|v| v == 0 || v > i64::MAX as u64)
        {
            return Err(StoreError::InvalidInput);
        }
        Recipient {
            address: input.sender.clone(),
            name: None,
        }
        .validate()
        .map_err(|_| StoreError::InvalidInput)?;
        if input.frozen.recipients.is_empty() || input.frozen.recipients.len() > 1000 {
            return Err(StoreError::InvalidInput);
        }
        for recipient in &input.frozen.recipients {
            Recipient {
                address: recipient.clone(),
                name: None,
            }
            .validate()
            .map_err(|_| StoreError::InvalidInput)?;
        }
        let (canonical, fingerprint) = send_identity(&input.draft_id, input.expected_version)?;
        self.execute(move|c|{
            let _permit=permit;
            let tx=c.transaction()?;
            let account=super::accounts::get(&tx,&input.account_id)?.ok_or(StoreError::NotFound)?;
            let id=uuid::Uuid::new_v4().to_string();
            let desired=json!({"draft_id":input.draft_id,"draft_version":input.snapshot_version,"submission":"send","sender":input.sender,"recipients":input.frozen.recipients,"message_id":input.frozen.message_id,"thread_id":input.frozen.thread_id});
            let inserted=tx.execute("INSERT INTO operations(account_id,id,request_id,kind,resource_id,fingerprint,request_json,desired_json,state,created_at_ms,updated_at_ms) VALUES (?1,?2,?3,'send',?4,?5,?6,?7,'queued',?8,?8) ON CONFLICT(account_id,request_id) DO NOTHING",params![input.account_id,id,request_id,input.draft_id,fingerprint,canonical,desired.to_string(),now])?;
            if inserted==0 {
                let id:String=tx.query_row("SELECT id FROM operations WHERE account_id=?1 AND request_id=?2",params![input.account_id,request_id],|r|r.get(0))?;
                let original=get(&tx,&input.account_id,&id)?;
                if original.fingerprint!=fingerprint {return Err(StoreError::VersionConflict);}
                tx.commit()?;return Ok(original);
            }
            if account.account.address!=input.sender {return Err(StoreError::VersionConflict);}
            let draft=super::drafts::get(&tx,&input.account_id,&input.draft_id)?;
            if draft.version!=input.snapshot_version || input.expected_version.is_some_and(|v|v!=draft.version) {return Err(StoreError::VersionConflict);}
            let wire=super::blobs::insert(&tx,&input.account_id,&input.frozen.wire)?;
            let sent=input.frozen.sent_copy.as_ref().map(|b|super::blobs::insert(&tx,&input.account_id,b)).transpose()?;
            tx.execute("INSERT INTO send_payloads(account_id,operation_id,draft_id,draft_version,sender,recipients_json,message_id,thread_id,wire_blob_id,sent_blob_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![input.account_id,id,input.draft_id,input.snapshot_version as i64,input.sender,serde_json::to_string(&input.frozen.recipients).map_err(|_|StoreError::InvalidInput)?,input.frozen.message_id,input.frozen.thread_id,wire.id,sent.map(|b|b.id)])?;
            if account.account.provider=="imap" { super::smtp::capture(&tx,&input.account_id,&id,input.frozen.sent_copy.as_deref().unwrap_or(&input.frozen.wire))?; }
            super::changes::record(&tx,Some(&input.account_id),"operation",Some(&id))?;
            let result=get(&tx,&input.account_id,&id)?;tx.commit()?;Ok(result)
        }).await
    }
    pub async fn get_operation(
        &self,
        account: String,
        id: String,
    ) -> Result<Operation, StoreError> {
        self.execute(move |c| get(c, &account, &id)).await
    }
    pub async fn send_payload(
        &self,
        account: String,
        id: String,
    ) -> Result<SendPayload, StoreError> {
        self.execute(move|c|{
            let (draft_id,version,sender,recipients,message_id,thread_id,wire,sent):(String,i64,String,String,String,Option<String>,String,Option<String>)=c.query_row("SELECT draft_id,draft_version,sender,recipients_json,message_id,thread_id,wire_blob_id,sent_blob_id FROM send_payloads WHERE account_id=?1 AND operation_id=?2",params![account,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?.ok_or(StoreError::NotFound)?;
            let blob=|id:String|c.query_row("SELECT byte_length,sha256 FROM blobs WHERE account_id=?1 AND id=?2",params![account,id],|r|Ok(StoredBlob {id:id.clone(),byte_length:u64::from(r.get::<_,u32>(0)?),sha256:r.get(1)?})).map_err(StoreError::from);
            Ok(SendPayload {draft_id,draft_version:u64::try_from(version).map_err(|_|StoreError::KeyOrCorrupt)?,sender,recipients:serde_json::from_str(&recipients).map_err(|_|StoreError::KeyOrCorrupt)?,message_id,thread_id,wire:blob(wire)?,sent_copy:sent.map(blob).transpose()?})
        }).await
    }
    pub async fn cancel_operation(
        &self,
        account: String,
        id: String,
        now: i64,
    ) -> Result<Operation, StoreError> {
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;let operation=get(&tx,&account,&id)?;
            if operation.state=="cancelled" {return Ok(operation);}
            if operation.state!="queued" {return Err(StoreError::VersionConflict);}
            tx.execute("UPDATE operations SET state='cancelled',version=version+1,updated_at_ms=?3 WHERE account_id=?1 AND id=?2",params![account,id,now.max(operation.updated_at_ms)])?;
            super::changes::record(&tx,Some(&account),"operation",Some(&id))?;
            let result=get(&tx,&account,&id)?;tx.commit()?;Ok(result)
        }).await
    }
}
fn send_identity(
    draft_id: &str,
    expected_version: Option<u64>,
) -> Result<(String, String), StoreError> {
    if draft_id.is_empty()
        || draft_id.len() > 128
        || expected_version.is_some_and(|v| v == 0 || v > i64::MAX as u64)
    {
        return Err(StoreError::InvalidInput);
    }
    let canonical = serde_json::to_string(
        &json!({"version":1,"kind":"send","draft_id":draft_id,"expected_version":expected_version}),
    )
    .map_err(|_| StoreError::InvalidInput)?;
    let fingerprint = hex::encode(Sha256::digest(canonical.as_bytes()));
    Ok((canonical, fingerprint))
}
pub(super) fn get(c: &Connection, account: &str, id: &str) -> Result<Operation, StoreError> {
    let (mut operation,desired):(Operation,String)=c.query_row("SELECT request_id,kind,resource_id,fingerprint,state,version,created_at_ms,updated_at_ms,next_attempt_at_ms,needs_reconciliation,error_code,disposition,desired_json FROM operations WHERE account_id=?1 AND id=?2",params![account,id],|r|Ok((Operation {id:id.into(),account_id:account.into(),request_id:r.get(0)?,kind:r.get(1)?,resource_id:r.get(2)?,fingerprint:r.get(3)?,state:r.get(4)?,version:u64::try_from(r.get::<_,i64>(5)?).map_err(|_|rusqlite::Error::InvalidQuery)?,created_at_ms:r.get(6)?,updated_at_ms:r.get(7)?,next_attempt_at_ms:r.get(8)?,needs_reconciliation:r.get(9)?,error_code:r.get(10)?,disposition:r.get(11)?,desired_state:Value::Null,resolutions:Vec::new(),reconciliation:None},r.get(12)?))).optional()?.ok_or(StoreError::NotFound)?;
    operation.desired_state =
        serde_json::from_str(&desired).map_err(|_| StoreError::KeyOrCorrupt)?;
    operation.resolutions = super::operation_resolutions::read(c, account, id)?;
    operation.reconciliation = super::operation_reconciliation::read(c, account, id)?;
    Ok(operation)
}
