use super::{operations, Operation, Store, StoreError};
use crate::domain::{calendar::CalendarObject, calendar_change::CalendarAction};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

mod receipt;
pub(super) use receipt::{apply, observe};
pub use receipt::{CalendarChangeReceipt, CalendarRetryEvidence};

#[derive(Clone, Serialize)]
pub struct EnqueueCalendarChange {
    pub account_id: String,
    pub calendar_id: String,
    pub request_id: String,
    pub action: Value,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarWriteKind {
    Create,
    Update,
    Delete,
    Respond,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalendarChangePayload {
    pub kind: CalendarWriteKind,
    pub provider_calendar_id: String,
    pub provider_event_id: String,
    pub local_event_id: String,
    pub time_zone: String,
    pub expected_etag: Option<String>,
    pub notifications: String,
    pub patch: Value,
    pub base: Option<Value>,
}
impl Store {
    pub async fn enqueue_calendar_change(
        &self,
        mut input: EnqueueCalendarChange,
        now: i64,
    ) -> Result<Operation, StoreError> {
        input.request_id = uuid::Uuid::parse_str(&input.request_id)
            .map_err(|_| StoreError::InvalidInput)?
            .to_string();
        if now < 0
            || [&input.account_id, &input.calendar_id]
                .iter()
                .any(|s| s.is_empty() || s.len() > 128)
        {
            return Err(StoreError::InvalidInput);
        }
        let request=serde_json::to_string(&json!({"kind":"calendar_change","calendar_id":input.calendar_id,"action":input.action})).map_err(|_|StoreError::InvalidInput)?;
        if request.len() > 1024 * 1024 {
            return Err(StoreError::InvalidInput);
        }
        let fingerprint = hex::encode(Sha256::digest(request.as_bytes()));
        self.execute(move|c|{
            let tx=c.transaction()?;
            let prior:Option<String>=tx.query_row("SELECT id FROM operations WHERE account_id=?1 AND request_id=?2",params![input.account_id,input.request_id],|r|r.get(0)).optional()?;
            if let Some(id)=prior {
                let op=operations::get(&tx,&input.account_id,&id)?;
                if op.kind!="calendar_change" || op.fingerprint!=fingerprint {return Err(StoreError::VersionConflict);}
                return Ok(op);
            }
            let account=super::accounts::get(&tx,&input.account_id)?.ok_or(StoreError::NotFound)?;
            if account.account.provider!="google" {return Err(StoreError::InvalidInput);}
            let (provider_calendar_id,zone,role):(String,Option<String>,String)=tx.query_row("SELECT provider_id,time_zone,access_role FROM calendars WHERE account_id=?1 AND id=?2 AND retired=0",params![input.account_id,input.calendar_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or(StoreError::NotFound)?;
            let time_zone=zone.ok_or(StoreError::InvalidInput)?;
            let action=CalendarAction::from_value(input.action,&time_zone).map_err(|_|StoreError::InvalidInput)?;
            let (local_event_id,provider_event_id,expected_etag,base)=if let Some((id,etag))=action.identity() {
                let (provider,encoded):(String,String)=tx.query_row("SELECT i.provider_id,coalesce(c.object_json,o.object_json) FROM calendar_event_ids i LEFT JOIN calendar_objects c USING(account_id,calendar_id,provider_id) LEFT JOIN calendar_occurrences o USING(account_id,calendar_id,provider_id) WHERE i.account_id=?1 AND i.calendar_id=?2 AND i.id=?3 AND (c.object_json IS NOT NULL OR o.object_json IS NOT NULL)",params![input.account_id,input.calendar_id,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(StoreError::NotFound)?;
                let object:CalendarObject=serde_json::from_str(&encoded).map_err(|_|StoreError::KeyOrCorrupt)?;
                let base:Value=serde_json::from_str(&object.provider_json).map_err(|_|StoreError::KeyOrCorrupt)?;
                (id.into(),provider,Some(etag.into()),Some(base))
            } else {(uuid::Uuid::new_v4().to_string(),uuid::Uuid::new_v4().simple().to_string(),None,None)};
            let mut patch=action.provider_patch(base.as_ref(),&time_zone,&account.account.address,&role).map_err(|_|StoreError::InvalidInput)?;
            let kind=match action {CalendarAction::Create{..}=>CalendarWriteKind::Create,CalendarAction::Update{..}=>CalendarWriteKind::Update,CalendarAction::Delete{..}=>CalendarWriteKind::Delete,CalendarAction::Respond{..}=>CalendarWriteKind::Respond};
            if kind==CalendarWriteKind::Create {patch["id"]=json!(provider_event_id);}
            let payload=CalendarChangePayload{kind,provider_calendar_id:provider_calendar_id.clone(),provider_event_id:provider_event_id.clone(),local_event_id:local_event_id.clone(),time_zone,expected_etag,notifications:action.notification_parameter().into(),patch,base};
            let encoded=serde_json::to_string(&payload).map_err(|_|StoreError::InvalidInput)?;
            if encoded.len()>4*1024*1024 {return Err(StoreError::ResultTooLarge);}
            let mut desired=serde_json::to_value(&payload).map_err(|_|StoreError::InvalidInput)?;
            if let Some(object)=desired.as_object_mut() {object.remove("base");}
            let desired=serde_json::to_string(&desired).map_err(|_|StoreError::InvalidInput)?;
            let id=uuid::Uuid::new_v4().to_string();
            tx.execute("INSERT INTO operations(account_id,id,request_id,kind,resource_id,fingerprint,request_json,desired_json,state,created_at_ms,updated_at_ms) VALUES (?1,?2,?3,'calendar_change',?4,?5,?6,?7,'queued',?8,?8)",params![input.account_id,id,input.request_id,local_event_id,fingerprint,request,desired,now])?;
            tx.execute("INSERT INTO calendar_change_payloads(account_id,operation_id,provider_calendar_id,provider_event_id,payload_json) VALUES (?1,?2,?3,?4,?5)",params![input.account_id,id,provider_calendar_id,provider_event_id,encoded])?;
            super::changes::record(&tx,Some(&input.account_id),"operation",Some(&id))?;
            let result=operations::get(&tx,&input.account_id,&id)?;tx.commit()?;Ok(result)
        }).await
    }
    pub async fn calendar_change_payload(
        &self,
        account: String,
        id: String,
    ) -> Result<CalendarChangePayload, StoreError> {
        self.execute(move |c| payload(c, &account, &id)).await
    }
}
pub(super) fn payload(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<CalendarChangePayload, StoreError> {
    let value:String=c.query_row("SELECT payload_json FROM calendar_change_payloads WHERE account_id=?1 AND operation_id=?2",params![account,id],|r|r.get(0)).optional()?.ok_or(StoreError::NotFound)?;
    serde_json::from_str(&value).map_err(|_| StoreError::KeyOrCorrupt)
}
pub(super) fn blocked(c: &Connection, account: &str, id: &str) -> Result<bool, StoreError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM calendar_change_payloads current JOIN calendar_change_payloads prior ON prior.account_id=current.account_id AND prior.provider_calendar_id=current.provider_calendar_id AND prior.provider_event_id=current.provider_event_id AND prior.sequence<current.sequence JOIN operations o ON o.account_id=prior.account_id AND o.id=prior.operation_id WHERE current.account_id=?1 AND current.operation_id=?2 AND o.state NOT IN ('applied','failed','cancelled') AND o.disposition IS NULL)",params![account,id],|r|r.get(0))?)
}
