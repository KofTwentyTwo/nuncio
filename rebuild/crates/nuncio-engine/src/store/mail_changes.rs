use super::{operations, Operation, Store, StoreError};
use crate::domain::mail_change::MailAction;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize)]
pub struct EnqueueMailChange {
    pub account_id: String,
    pub message_id: String,
    pub request_id: String,
    pub action: MailAction,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailChangePayload {
    pub provider_message_id: String,
    pub endpoint: String,
    pub add_label_ids: Vec<String>,
    pub remove_label_ids: Vec<String>,
}
pub struct MailChangeReceipt {
    pub provider_message_id: String,
    pub label_ids: Vec<String>,
    pub history_id: Option<String>,
}
impl MailChangeReceipt {
    pub fn from_google(value: &serde_json::Value) -> Result<Self, StoreError> {
        let result = Self {
            provider_message_id: value["id"].as_str().ok_or(StoreError::InvalidInput)?.into(),
            label_ids: match value.get("labelIds") {
                None => Vec::new(),
                Some(value) => value
                    .as_array()
                    .ok_or(StoreError::InvalidInput)?
                    .iter()
                    .map(|v| v.as_str().map(String::from).ok_or(StoreError::InvalidInput))
                    .collect::<Result<_, _>>()?,
            },
            history_id: value
                .get("historyId")
                .map(|v| v.as_str().map(String::from).ok_or(StoreError::InvalidInput))
                .transpose()?,
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), StoreError> {
        if !provider_id(&self.provider_message_id)
            || self.label_ids.len() > 4096
            || self.label_ids.iter().any(|id| !provider_id(id))
            || self
                .label_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.label_ids.len()
            || self.history_id.as_ref().is_some_and(|h| {
                h.is_empty() || h.len() > 40 || !h.bytes().all(|b| b.is_ascii_digit())
            })
        {
            return Err(StoreError::InvalidInput);
        }
        Ok(())
    }
}
impl MailChangePayload {
    pub fn satisfied_by(&self, receipt: &MailChangeReceipt) -> bool {
        self.provider_message_id == receipt.provider_message_id
            && self
                .add_label_ids
                .iter()
                .all(|id| receipt.label_ids.contains(id))
            && self
                .remove_label_ids
                .iter()
                .all(|id| !receipt.label_ids.contains(id))
    }
}
impl Store {
    pub async fn enqueue_mail_change(
        &self,
        mut input: EnqueueMailChange,
        now: i64,
    ) -> Result<Operation, StoreError> {
        input.request_id = uuid::Uuid::parse_str(&input.request_id)
            .map_err(|_| StoreError::InvalidInput)?
            .to_string();
        if now < 0
            || input.account_id.is_empty()
            || input.account_id.len() > 128
            || input.message_id.is_empty()
            || input.message_id.len() > 128
        {
            return Err(StoreError::InvalidInput);
        }
        if let MailAction::Label { collection_id, .. }
        | MailAction::Move {
            destination_collection_id: collection_id,
        }
        | MailAction::Copy {
            destination_collection_id: collection_id,
        } = &input.action
        {
            if collection_id.is_empty() || collection_id.len() > 128 {
                return Err(StoreError::InvalidInput);
            }
        }
        let request=serde_json::to_string(&serde_json::json!({"kind":"mail_change","message_id":input.message_id,"action":input.action})).map_err(|_|StoreError::InvalidInput)?;
        let fingerprint = hex::encode(Sha256::digest(request.as_bytes()));
        self.execute(move|c|{
            let tx=c.transaction()?;
            let prior:Option<String>=tx.query_row("SELECT id FROM operations WHERE account_id=?1 AND request_id=?2",params![input.account_id,input.request_id],|r|r.get(0)).optional()?;
            if let Some(id)=prior {
                let op=operations::get(&tx,&input.account_id,&id)?;
                if op.kind!="mail_change" || op.fingerprint!=fingerprint {return Err(StoreError::VersionConflict);}
                return Ok(op);
            }
            let account=super::accounts::get(&tx,&input.account_id)?.ok_or(StoreError::NotFound)?;
            if !matches!(account.account.provider.as_str(),"google"|"imap") {return Err(StoreError::InvalidInput);}
            let provider:String=tx.query_row("SELECT provider_id FROM messages WHERE account_id=?1 AND id=?2",params![input.account_id,input.message_id],|r|r.get(0)).optional()?.ok_or(StoreError::NotFound)?;
            if !provider_id(&provider) {return Err(StoreError::InvalidInput);}
            let desired=if account.account.provider=="imap" {
                serde_json::to_string(&super::imap_changes::capture(&tx,&input,&provider)?).map_err(|_|StoreError::InvalidInput)?
            }else{
            let (endpoint,label,present)=match &input.action {
                MailAction::Read{read}=>("modify","UNREAD".into(),!read),
                MailAction::Star{starred}=>("modify","STARRED".into(),*starred),
                MailAction::Archive {}=>("modify","INBOX".into(),false),
                MailAction::Move {..} | MailAction::Copy {..} => return Err(StoreError::InvalidInput),
                MailAction::Trash{trashed}=>(if *trashed{"trash"}else{"untrash"},"TRASH".into(),*trashed),
                MailAction::Label{collection_id,present}=>{
                    let label:String=tx.query_row("SELECT provider_id FROM collections WHERE account_id=?1 AND id=?2 AND kind='user' AND retired=0",params![input.account_id,collection_id],|r|r.get(0)).optional()?.ok_or(StoreError::NotFound)?;
                    if !provider_id(&label) {return Err(StoreError::InvalidInput);}
                    ("modify",label,*present)
                },
            };
            let payload=MailChangePayload{provider_message_id:provider.clone(),endpoint:endpoint.into(),add_label_ids:if present{vec![label.clone()]}else{vec![]},remove_label_ids:if present{vec![]}else{vec![label]}};
            serde_json::to_string(&payload).map_err(|_|StoreError::InvalidInput)?
            };
            let id=uuid::Uuid::new_v4().to_string();
            tx.execute("INSERT INTO operations(account_id,id,request_id,kind,resource_id,fingerprint,request_json,desired_json,state,created_at_ms,updated_at_ms) VALUES (?1,?2,?3,'mail_change',?4,?5,?6,?7,'queued',?8,?8)",params![input.account_id,id,input.request_id,input.message_id,fingerprint,request,desired,now])?;
            tx.execute("INSERT INTO mail_change_payloads(account_id,operation_id,provider_message_id,payload_json) VALUES (?1,?2,?3,?4)",params![input.account_id,id,provider,desired])?;
            super::changes::record(&tx,Some(&input.account_id),"operation",Some(&id))?;
            let result=operations::get(&tx,&input.account_id,&id)?;tx.commit()?;Ok(result)
        }).await
    }
    pub async fn mail_change_payload(
        &self,
        account: String,
        id: String,
    ) -> Result<MailChangePayload, StoreError> {
        self.execute(move |c| payload(c, &account, &id)).await
    }
}
pub(super) fn payload(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<MailChangePayload, StoreError> {
    let value: String = c
        .query_row(
            "SELECT payload_json FROM mail_change_payloads WHERE account_id=?1 AND operation_id=?2",
            params![account, id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    serde_json::from_str(&value).map_err(|_| StoreError::KeyOrCorrupt)
}
pub(super) fn blocked(c: &Connection, account: &str, id: &str) -> Result<bool, StoreError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM mail_change_payloads current JOIN mail_change_payloads prior ON prior.account_id=current.account_id AND prior.provider_message_id=current.provider_message_id AND prior.sequence<current.sequence JOIN operations o ON o.account_id=prior.account_id AND o.id=prior.operation_id WHERE current.account_id=?1 AND current.operation_id=?2 AND o.state NOT IN ('applied','failed','cancelled') AND o.disposition IS NULL)",params![account,id],|r|r.get(0))?)
}
pub(super) fn apply(
    c: &Connection,
    account: &str,
    id: &str,
    receipt: &MailChangeReceipt,
) -> Result<(), StoreError> {
    receipt.validate()?;
    if !payload(c, account, id)?.satisfied_by(receipt) {
        return Err(StoreError::InvalidInput);
    }
    // A projection reset can replace the local ID. Intent retains provider identity.
    let local: Option<String> = c
        .query_row(
            "SELECT id FROM messages WHERE account_id=?1 AND provider_id=?2",
            params![account, receipt.provider_message_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(local) = local {
        c.execute(
            "DELETE FROM memberships WHERE account_id=?1 AND message_id=?2",
            params![account, local],
        )?;
        for label in &receipt.label_ids {
            c.execute("INSERT INTO collections(account_id,id,provider_id,kind) VALUES (?1,?2,?3,'unknown') ON CONFLICT(account_id,provider_id) DO UPDATE SET retired=0",params![account,uuid::Uuid::new_v4().to_string(),label])?;
            c.execute("INSERT INTO memberships SELECT account_id,?2,id FROM collections WHERE account_id=?1 AND provider_id=?3",params![account,local,label])?;
        }
        let labels =
            serde_json::to_string(&receipt.label_ids).map_err(|_| StoreError::InvalidInput)?;
        c.execute("UPDATE messages SET history_id=coalesce(?3,history_id),provider_json=json_set(provider_json,'$.labelIds',json(?4)) WHERE account_id=?1 AND id=?2",params![account,local,receipt.history_id,labels])?;
        if let Some(history) = &receipt.history_id {
            c.execute("UPDATE messages SET provider_json=json_set(provider_json,'$.historyId',?3) WHERE account_id=?1 AND id=?2",params![account,local,history])?;
        }
        super::changes::record(c, Some(account), "mail", Some(&local))?;
    }
    Ok(())
}
fn provider_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 2048 && !id.chars().any(char::is_control)
}
