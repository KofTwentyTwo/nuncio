use super::{operations, Operation, Store, StoreError};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AttemptKind {
    Dispatch,
    Reconcile,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationReceipt {
    pub kind: String,
    pub source: String,
    pub provider_id: Option<String>,
    pub etag: Option<String>,
}
impl OperationReceipt {
    pub(super) fn validate(&self) -> Result<(), StoreError> {
        if !matches!(
            self.kind.as_str(),
            "google_send"
                | "mail_change"
                | "calendar_change"
                | "calendar_retry"
                | "calendar_conflict"
                | "smtp_accepted"
                | "smtp_rejected"
                | "imap_append"
                | "imap_append_rejected"
                | "server_sent_observed"
                | "sent_copy"
                | "imap_change"
                | "imap_copy"
        ) || !matches!(
            self.source.as_str(),
            "acknowledgement" | "positive_read" | "manual_confirmation"
        ) || (!matches!(self.kind.as_str(), "smtp_accepted" | "smtp_rejected")
            && self.provider_id.is_none())
            || [&self.provider_id, &self.etag]
                .into_iter()
                .flatten()
                .any(|s| s.is_empty() || s.len() > 2048 || s.chars().any(char::is_control))
        {
            return Err(StoreError::InvalidInput);
        }
        Ok(())
    }
}
pub enum AttemptOutcome {
    RetryUnstartedSmtp {
        retry_at_ms: i64,
    },
    SmtpDataRejected {
        reply_code: u16,
        retry_at_ms: Option<i64>,
    },
    ObservedClientSent {
        result: Box<super::SentCopyResult>,
    },
    AppliedSentCopy {
        result: Box<super::SentCopyResult>,
    },
    ObservedCalendarConflict {
        code: String,
        observed: serde_json::Value,
    },
    RepeatableCalendar {
        code: String,
        retry_at_ms: i64,
        evidence: super::CalendarRetryEvidence,
    },
    ReconcileLater {
        code: String,
        retry_at_ms: i64,
    },
    AppliedCalendar {
        source: String,
        receipt: super::CalendarChangeReceipt,
    },
    RetryUnstartedImapTransfer {
        retry_at_ms: i64,
    },
    AppliedImapTransfer {
        source: String,
        result: Box<super::ImapTransferResult>,
    },
    AppliedImapFlags {
        source: String,
        message: crate::domain::imap::ImapMessageState,
    },
    AppliedMail {
        source: String,
        message: super::MailChangeReceipt,
    },
    Applied(OperationReceipt),
    /// The executor positively established that dispatch was rejected. A failed
    /// reconciliation read is never this proof about an earlier submission.
    Rejected {
        code: String,
        retry_at_ms: Option<i64>,
    },
    Repeatable {
        code: String,
        retry_at_ms: i64,
    },
    Conflict {
        code: String,
    },
    Uncertain {
        code: String,
    },
}
#[derive(Clone, Serialize)]
pub struct OperationAttempt {
    pub ordinal: u32,
    pub kind: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub outcome: Option<String>,
    pub error_code: Option<String>,
    pub receipts: Vec<RecordedReceipt>,
}
#[derive(Clone, Serialize)]
pub struct RecordedReceipt {
    pub kind: String,
    pub source: String,
    pub provider_id: Option<String>,
    pub etag: Option<String>,
    pub observed_at_ms: i64,
}
impl Store {
    /// Persist the final SMTP success response before attempting an IMAP copy.
    pub async fn record_smtp_acceptance(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        queue_id: Option<String>,
        now: i64,
    ) -> Result<Operation, StoreError> {
        let receipt = OperationReceipt {
            kind: "smtp_accepted".into(),
            source: "acknowledgement".into(),
            provider_id: queue_id,
            etag: None,
        };
        receipt.validate()?;
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move |c| {
            let tx=c.transaction()?;
            let op=operations::get(&tx,&account,&id)?;
            let provider=super::accounts::get(&tx,&account)?.ok_or(StoreError::NotFound)?.account.provider;
            if op.kind!="send" || provider!="imap" { return Err(StoreError::InvalidInput); }
            let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM operation_attempts WHERE account_id=?1 AND operation_id=?2 AND ordinal=?3 AND kind='dispatch' AND finished_at_ms IS NULL)",params![account,id,ordinal],|r|r.get(0))?;
            if op.state!="running" || !valid { return Err(StoreError::VersionConflict); }
            if smtp_accepted(&tx,&account,&id)? || super::smtp::progress(&tx,&account,&id)?.step!=super::SmtpStep::Started { return Err(StoreError::VersionConflict); }
            tx.execute("UPDATE smtp_submissions SET step='accepted' WHERE account_id=?1 AND operation_id=?2",params![account,id])?;
            let now=now.max(op.updated_at_ms);
            append_receipt(&tx,&account,&id,ordinal,&receipt,now)?;
            tx.execute("UPDATE operations SET version=version+1,updated_at_ms=?3 WHERE account_id=?1 AND id=?2",params![account,id,now])?;
            super::changes::record(&tx,Some(&account),"operation",Some(&id))?;
            let result=operations::get(&tx,&account,&id)?;
            tx.commit()?;Ok(result)
        }).await
    }
    pub async fn begin_operation_attempt(
        &self,
        account: String,
        id: String,
        kind: AttemptKind,
        now: i64,
    ) -> Result<OperationAttempt, StoreError> {
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;let op=operations::get(&tx,&account,&id)?;
            if kind==AttemptKind::Dispatch && !super::operation_reconciliation::dispatch_allowed(&tx,&op)? {return Err(StoreError::VersionConflict);}
            let admissible=match kind {AttemptKind::Dispatch=>matches!(op.state.as_str(),"queued"|"retry_wait") && op.next_attempt_at_ms.is_none_or(|t|now>=t),AttemptKind::Reconcile=>op.state=="uncertain"&&op.needs_reconciliation&&op.next_attempt_at_ms.is_none_or(|t|now>=t)};
            if !admissible || op.disposition.is_some() || (op.kind=="mail_change" && super::mail_changes::blocked(&tx,&account,&id)?) || (op.kind=="calendar_change" && super::calendar_changes::blocked(&tx,&account,&id)?) {return Err(StoreError::VersionConflict);}
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM operation_attempts WHERE account_id=?1 AND finished_at_ms IS NULL)",[&account],|r|r.get::<_,bool>(0))? {return Err(StoreError::Busy);}
            let ordinal:u32=tx.query_row("SELECT COALESCE(max(ordinal),0)+1 FROM operation_attempts WHERE account_id=?1 AND operation_id=?2",params![account,id],|r|r.get(0))?;
            if ordinal>1000 {return Err(StoreError::ResultTooLarge);}
            let kind=match kind {AttemptKind::Dispatch=>"dispatch",AttemptKind::Reconcile=>"reconcile"};
            if kind=="reconcile" {
                if let Some(request)=super::operation_reconciliation::read(&tx,&account,&id)?.filter(|r|r.active&&r.first_attempt_ordinal.is_none()) {
                    tx.execute("UPDATE operation_reconciliation_requests SET first_attempt_ordinal=?3 WHERE account_id=?1 AND request_id=?2",params![account,request.request_id,ordinal])?;
                }
            }
            let now=now.max(op.updated_at_ms);
            tx.execute("INSERT INTO operation_attempts(account_id,operation_id,ordinal,kind,started_at_ms) VALUES (?1,?2,?3,?4,?5)",params![account,id,ordinal,kind,now])?;
            tx.execute("UPDATE operations SET state=CASE ?3 WHEN 'dispatch' THEN 'running' ELSE 'uncertain' END,version=version+1,updated_at_ms=?4,next_attempt_at_ms=NULL WHERE account_id=?1 AND id=?2",params![account,id,kind,now])?;
            super::changes::record(&tx,Some(&account),"operation",Some(&id))?;
            tx.commit()?;Ok(OperationAttempt {ordinal,kind:kind.into(),started_at_ms:now,finished_at_ms:None,outcome:None,error_code:None,receipts:Vec::new()})
        }).await
    }
    pub async fn finish_operation_attempt(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        outcome: AttemptOutcome,
        now: i64,
    ) -> Result<Operation, StoreError> {
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;let op=operations::get(&tx,&account,&id)?;
            let (kind,finished):(String,Option<i64>)=tx.query_row("SELECT kind,finished_at_ms FROM operation_attempts WHERE account_id=?1 AND operation_id=?2 AND ordinal=?3",params![account,id,ordinal],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if finished.is_some() || !matches!((kind.as_str(),op.state.as_str()),("dispatch","running")|("reconcile","uncertain")) {return Err(StoreError::VersionConflict);}
            let now=now.max(op.updated_at_ms);
            let outcome=super::operation_reconciliation::constrain(&tx,&op,outcome)?;
            let (state,result,error,next,reconcile)=match outcome {
                AttemptOutcome::RetryUnstartedSmtp{retry_at_ms}=>{
                    if kind!="reconcile" || retry_at_ms<0 || super::smtp::progress(&tx,&account,&id)?.step!=super::SmtpStep::Prepared {return Err(StoreError::InvalidInput);}
                    ("retry_wait","repeatable",Some("smtp_data_not_dispatched".into()),Some(retry_at_ms.max(now)),false)
                },
                AttemptOutcome::SmtpDataRejected{reply_code,retry_at_ms}=>{
                    if kind!="dispatch" || !(400..600).contains(&reply_code) || retry_at_ms.is_some_and(|t|t<0 || reply_code>=500) || super::smtp::progress(&tx,&account,&id)?.step!=super::SmtpStep::Started {return Err(StoreError::InvalidInput);}
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"smtp_rejected".into(),source:"acknowledgement".into(),provider_id:None,etag:Some(reply_code.to_string())},now)?;
                    tx.execute("UPDATE smtp_submissions SET step='prepared',sent_floor=NULL WHERE account_id=?1 AND operation_id=?2",params![account,id])?;
                    (if retry_at_ms.is_some(){"retry_wait"}else{"failed"},"rejected",Some(format!("smtp_data_{reply_code}")),retry_at_ms.map(|t|t.max(now)),false)
                },
                AttemptOutcome::ObservedClientSent{result}=>{
                    if op.kind!="send" || kind!="reconcile" || super::operation_reconciliation::policy(&tx,&account,&id)?.mode.is_none() || !smtp_accepted(&tx,&account,&id)? {return Err(StoreError::InvalidInput);}
                    let provider_id=super::smtp::apply_client_observation(&tx,&account,&id,*result)?;
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"sent_copy".into(),source:"positive_read".into(),provider_id:Some(provider_id),etag:None},now)?;
                    ("applied","applied",None,None,false)
                },
                AttemptOutcome::AppliedSentCopy{result}=>{
                    if op.kind!="send" || !smtp_accepted(&tx,&account,&id)? {return Err(StoreError::InvalidInput);}
                    let provider_id=super::smtp::apply(&tx,&account,&id,*result)?;
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"sent_copy".into(),source:"positive_read".into(),provider_id:Some(provider_id),etag:None},now)?;
                    ("applied","applied",None,None,false)
                },
                AttemptOutcome::ObservedCalendarConflict{code,observed}=>{
                    if op.kind!="calendar_change" {return Err(StoreError::InvalidInput);}
                    validate_code(&code)?;
                    let payload=super::calendar_changes::payload(&tx,&account,&id)?;
                    let (provider_id,etag)=super::calendar_changes::observe(&tx,&account,&payload,observed)?;
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"calendar_conflict".into(),source:"positive_read".into(),provider_id:Some(provider_id),etag},now)?;
                    ("conflict","conflict",Some(code),None,false)
                },
                AttemptOutcome::ReconcileLater{code,retry_at_ms}=>{
                    if kind!="reconcile" || retry_at_ms<0 {return Err(StoreError::InvalidInput);}
                    validate_code(&code)?;
                    ("uncertain","uncertain",Some(code),Some(retry_at_ms.max(now)),true)
                },
                AttemptOutcome::RepeatableCalendar{code,retry_at_ms,evidence}=>{
                    if op.kind!="calendar_change" || kind!="reconcile" || retry_at_ms<0 {return Err(StoreError::InvalidInput);}
                    validate_code(&code)?;
                    let payload=super::calendar_changes::payload(&tx,&account,&id)?;
                    if !payload.permits_retry(&evidence) {return Err(StoreError::InvalidInput);}
                    let etag=match evidence {super::CalendarRetryEvidence::Unchanged(value)=>value["etag"].as_str().map(String::from),super::CalendarRetryEvidence::Missing{..}=>None};
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"calendar_retry".into(),source:"positive_read".into(),provider_id:Some(payload.provider_event_id),etag},now)?;
                    ("retry_wait","repeatable",Some(code),Some(retry_at_ms.max(now)),false)
                },
                AttemptOutcome::AppliedCalendar{source,receipt}=>{
                    if op.kind!="calendar_change" || !matches!((kind.as_str(),source.as_str()),("dispatch","acknowledgement")|("reconcile","positive_read")) {return Err(StoreError::InvalidInput);}
                    let (provider_id,etag,notifications)=super::calendar_changes::apply(&tx,&account,&id,&receipt)?;
                    let unknown=notifications && source=="positive_read";
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"calendar_change".into(),source,provider_id:Some(provider_id),etag},now)?;
                    if unknown {("uncertain","uncertain",Some("calendar_notifications_unconfirmed".into()),None,false)}
                    else {("applied","applied",None,None,false)}
                },
                AttemptOutcome::RetryUnstartedImapTransfer{retry_at_ms}=>{
                    if op.kind!="mail_change" || kind!="reconcile" || retry_at_ms<0 || !matches!(super::imap_transfers::payload(&tx,&account,&id)?,super::ImapChangePayload::Transfer(_)) || tx.query_row("SELECT EXISTS(SELECT 1 FROM imap_transfer_progress WHERE account_id=?1 AND operation_id=?2)",params![account,id],|r|r.get::<_,bool>(0))? {return Err(StoreError::InvalidInput);}
                    ("retry_wait","repeatable",Some("imap_transfer_not_dispatched".into()),Some(retry_at_ms.max(now)),false)
                },
                AttemptOutcome::AppliedImapTransfer{source,result}=>{
                    let same=result.proof.source==result.proof.destination;
                    if op.kind!="mail_change" || (same && source!="positive_read") || !(matches!((kind.as_str(),source.as_str()),("dispatch","acknowledgement")|("reconcile","positive_read")) || (same && kind=="dispatch" && source=="positive_read")) {return Err(StoreError::InvalidInput);}
                    let provider=super::imap_transfers::apply(&tx,&account,&id,*result)?;
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"imap_change".into(),source,provider_id:Some(provider),etag:None},now)?;
                    ("applied","applied",None,None,false)
                },
                AttemptOutcome::AppliedImapFlags{source,message}=>{
                    if op.kind!="mail_change" || !matches!((kind.as_str(),source.as_str()),("dispatch","acknowledgement")|("dispatch","positive_read")|("reconcile","positive_read")) {return Err(StoreError::InvalidInput);}
                    let provider=super::imap_changes::apply(&tx,&account,&id,&message)?;
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"imap_change".into(),source,provider_id:Some(provider),etag:None},now)?;
                    ("applied","applied",None,None,false)
                },
                AttemptOutcome::AppliedMail{source,message}=>{
                    if op.kind!="mail_change" || !matches!(source.as_str(),"acknowledgement"|"positive_read") {return Err(StoreError::InvalidInput);}
                    super::mail_changes::apply(&tx,&account,&id,&message)?;
                    append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"mail_change".into(),source,provider_id:Some(message.provider_message_id),etag:None},now)?;
                    ("applied","applied",None,None,false)
                },
                AttemptOutcome::Applied(receipt)=>{
                    receipt.validate()?;
                    if matches!(op.kind.as_str(),"mail_change"|"calendar_change") {return Err(StoreError::InvalidInput);}
                    if receipt.source=="manual_confirmation" {return Err(StoreError::InvalidInput);}
                    let expected=if op.kind=="send" {
                        let account=super::accounts::get(&tx,&account)?.ok_or(StoreError::NotFound)?;
                        if account.account.provider=="google" {"google_send"} else {"sent_copy"}
                    }else{op.kind.as_str()};
                    if receipt.kind!=expected {return Err(StoreError::InvalidInput);}
                    if expected=="sent_copy" {return Err(StoreError::InvalidInput);}
                    append_receipt(&tx,&account,&id,ordinal,&receipt,now)?;
                    ("applied","applied",None,None,false)
                },
                AttemptOutcome::Rejected {code,retry_at_ms}=>{
                    if kind!="dispatch" {return Err(StoreError::InvalidInput);}
                    if tx.query_row("SELECT EXISTS(SELECT 1 FROM imap_transfer_progress WHERE account_id=?1 AND operation_id=?2)",params![account,id],|r|r.get::<_,bool>(0))? {return Err(StoreError::InvalidInput);}
                    if smtp_accepted(&tx,&account,&id)? || tx.query_row("SELECT EXISTS(SELECT 1 FROM smtp_submissions WHERE account_id=?1 AND operation_id=?2 AND step<>'prepared')",params![account,id],|r|r.get::<_,bool>(0))? {return Err(StoreError::InvalidInput);}
                    validate_code(&code)?;
                    if retry_at_ms.is_some_and(|t|t<0) {return Err(StoreError::InvalidInput);}
                    let retry_at_ms=retry_at_ms.map(|t|t.max(now));
                    (if retry_at_ms.is_some(){"retry_wait"}else{"failed"},"rejected",Some(code),retry_at_ms,false)
                },
                AttemptOutcome::Repeatable {code,retry_at_ms}=>{
                    if op.kind!="mail_change" || retry_at_ms<0 {return Err(StoreError::InvalidInput);}
                    let provider=super::accounts::get(&tx,&account)?.ok_or(StoreError::NotFound)?.account.provider;
                    if provider=="imap" && !matches!(super::imap_transfers::payload(&tx,&account,&id)?,super::ImapChangePayload::Flags(_)) {return Err(StoreError::InvalidInput);}
                    let retry_at_ms=retry_at_ms.max(now);
                    validate_code(&code)?;
                    ("retry_wait","repeatable",Some(code),Some(retry_at_ms),false)
                },
                AttemptOutcome::Conflict {code}=>{
                    if (kind!="dispatch" && !matches!(op.kind.as_str(),"mail_change"|"calendar_change")) || op.kind=="send" {return Err(StoreError::InvalidInput);}
                    validate_code(&code)?;("conflict","conflict",Some(code),None,false)
                },
                AttemptOutcome::Uncertain {code}=>{
                    validate_code(&code)?;("uncertain","uncertain",Some(code),None,kind=="dispatch")
                },
            };
            tx.execute("UPDATE operation_attempts SET finished_at_ms=?4,outcome=?5,error_code=?6 WHERE account_id=?1 AND operation_id=?2 AND ordinal=?3",params![account,id,ordinal,now,result,error])?;
            tx.execute("UPDATE operations SET state=?3,version=version+1,updated_at_ms=?4,error_code=?5,next_attempt_at_ms=?6,needs_reconciliation=?7 WHERE account_id=?1 AND id=?2",params![account,id,state,now,error,next,reconcile])?;
            super::changes::record(&tx,Some(&account),"operation",Some(&id))?;
            let result=operations::get(&tx,&account,&id)?;tx.commit()?;Ok(result)
        }).await
    }
    pub async fn operation_attempts(
        &self,
        account: String,
        id: String,
    ) -> Result<Vec<OperationAttempt>, StoreError> {
        self.execute(move |c| {
            operations::get(c, &account, &id)?;
            let mut attempts = read_attempts(c, &account, &id, None, 1000)?;
            attempts.reverse();
            Ok(attempts)
        })
        .await
    }
    pub async fn recover_operations(&self, now: i64) -> Result<u64, StoreError> {
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;
            let pending=tx.prepare("SELECT account_id,operation_id FROM operation_attempts WHERE finished_at_ms IS NULL ORDER BY account_id,operation_id")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
            for (account,id) in &pending {
                let op=operations::get(&tx,account,id)?;
                if !matches!(op.state.as_str(),"running"|"uncertain") {return Err(StoreError::KeyOrCorrupt);}
                let now=now.max(op.updated_at_ms);
                tx.execute("UPDATE operation_attempts SET finished_at_ms=max(?3,started_at_ms),outcome='uncertain',error_code='interrupted' WHERE account_id=?1 AND operation_id=?2 AND finished_at_ms IS NULL",params![account,id,now])?;
                tx.execute("UPDATE operations SET state='uncertain',needs_reconciliation=1,version=version+1,updated_at_ms=?3,next_attempt_at_ms=NULL,error_code='interrupted' WHERE account_id=?1 AND id=?2",params![account,id,now])?;
                super::changes::record(&tx,Some(account),"operation",Some(id))?;
            }
            tx.commit()?;Ok(pending.len()as u64)
        }).await
    }
}
fn validate_code(code: &str) -> Result<(), StoreError> {
    if code.is_empty()
        || code.len() > 80
        || !code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}
fn smtp_accepted(c: &Connection, account: &str, id: &str) -> Result<bool, StoreError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM operation_receipts WHERE account_id=?1 AND operation_id=?2 AND json_extract(receipt_json,'$.kind')='smtp_accepted')",params![account,id],|r|r.get(0))?)
}
pub(super) fn read_attempts(
    c: &Connection,
    account: &str,
    id: &str,
    before: Option<u32>,
    limit: u32,
) -> Result<Vec<OperationAttempt>, StoreError> {
    let mut stmt=c.prepare("SELECT ordinal,kind,started_at_ms,finished_at_ms,outcome,error_code FROM operation_attempts WHERE account_id=?1 AND operation_id=?2 AND (?3 IS NULL OR ordinal<?3) ORDER BY ordinal DESC LIMIT ?4")?;
    let mut attempts = stmt
        .query_map(params![account, id, before, limit], |r| {
            Ok(OperationAttempt {
                ordinal: r.get(0)?,
                kind: r.get(1)?,
                started_at_ms: r.get(2)?,
                finished_at_ms: r.get(3)?,
                outcome: r.get(4)?,
                error_code: r.get(5)?,
                receipts: Vec::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for attempt in &mut attempts {
        let mut stmt=c.prepare("SELECT receipt_json,observed_at_ms FROM operation_receipts WHERE account_id=?1 AND operation_id=?2 AND attempt_ordinal=?3 ORDER BY sequence")?;
        let rows = stmt.query_map(params![account, id, attempt.ordinal], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (json, observed_at_ms) = row?;
            let receipt: OperationReceipt =
                serde_json::from_str(&json).map_err(|_| StoreError::KeyOrCorrupt)?;
            receipt.validate().map_err(|_| StoreError::KeyOrCorrupt)?;
            attempt.receipts.push(RecordedReceipt {
                kind: receipt.kind,
                source: receipt.source,
                provider_id: receipt.provider_id,
                etag: receipt.etag,
                observed_at_ms,
            });
        }
    }
    Ok(attempts)
}
pub(super) fn append_receipt(
    c: &Connection,
    account: &str,
    id: &str,
    ordinal: u32,
    receipt: &OperationReceipt,
    now: i64,
) -> Result<(), StoreError> {
    receipt.validate()?;
    let sequence:u32=c.query_row("SELECT COALESCE(max(sequence),0)+1 FROM operation_receipts WHERE account_id=?1 AND operation_id=?2 AND attempt_ordinal=?3",params![account,id,ordinal],|r|r.get(0))?;
    if sequence > 16 {
        return Err(StoreError::ResultTooLarge);
    }
    c.execute("INSERT INTO operation_receipts(account_id,operation_id,attempt_ordinal,sequence,observed_at_ms,receipt_json) VALUES (?1,?2,?3,?4,?5,?6)",params![account,id,ordinal,sequence,now,serde_json::to_string(receipt).map_err(|_|StoreError::InvalidInput)?])?;
    Ok(())
}
