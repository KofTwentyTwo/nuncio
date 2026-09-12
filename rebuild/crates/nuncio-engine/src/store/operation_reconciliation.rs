use super::{operations, AttemptOutcome, Operation, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationMode {
    Observe,
    ResumeSafe,
}
impl ReconciliationMode {
    fn value(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::ResumeSafe => "resume_safe",
        }
    }
    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "observe" => Ok(Self::Observe),
            "resume_safe" => Ok(Self::ResumeSafe),
            _ => Err(StoreError::KeyOrCorrupt),
        }
    }
}
#[derive(Clone)]
pub struct ReconcileOperation {
    pub account_id: String,
    pub operation_id: String,
    pub request_id: String,
    pub expected_version: u64,
    pub mode: ReconciliationMode,
}
#[derive(Clone, Serialize)]
pub struct ReconciliationRequest {
    pub request_id: String,
    pub expected_version: u64,
    pub mode: ReconciliationMode,
    pub requested_at_ms: i64,
    pub first_attempt_ordinal: Option<u32>,
    /// A request restored from an older snapshot is audit history, not permission to run.
    pub active: bool,
}
#[derive(Clone, Copy)]
pub(crate) struct ExecutionPolicy {
    pub restored: bool,
    pub mode: Option<ReconciliationMode>,
    first_attempt_ordinal: Option<u32>,
}
impl ExecutionPolicy {
    pub fn retry_ordinal(self, ordinal: u32) -> Result<u32, StoreError> {
        ordinal
            .checked_sub(self.first_attempt_ordinal.unwrap_or(1))
            .and_then(|value| value.checked_add(1))
            .ok_or(StoreError::KeyOrCorrupt)
    }
    pub fn observes_only(self) -> bool {
        self.mode == Some(ReconciliationMode::Observe) || (self.restored && self.mode.is_none())
    }
}
fn source_version(c: &Connection, account: &str, id: &str) -> Result<Option<u64>, StoreError> {
    let value: Option<i64> = c.query_row("SELECT max(source_version) FROM restored_operations WHERE account_id=?1 AND operation_id=?2", params![account,id],|r|r.get(0))?;
    value
        .map(|v| u64::try_from(v).map_err(|_| StoreError::KeyOrCorrupt))
        .transpose()
}
pub(super) fn read(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<Option<ReconciliationRequest>, StoreError> {
    let row=c.query_row("SELECT request_id,expected_version,mode,requested_at_ms,first_attempt_ordinal FROM operation_reconciliation_requests WHERE account_id=?1 AND operation_id=?2 ORDER BY expected_version DESC LIMIT 1",params![account,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,Option<u32>>(4)?))).optional()?;
    let Some((request_id, version, mode, requested_at_ms, first_attempt_ordinal)) = row else {
        return Ok(None);
    };
    let expected_version = u64::try_from(version).map_err(|_| StoreError::KeyOrCorrupt)?;
    Ok(Some(ReconciliationRequest {
        request_id,
        expected_version,
        mode: ReconciliationMode::parse(&mode)?,
        requested_at_ms,
        first_attempt_ordinal,
        active: source_version(c, account, id)?.is_none_or(|source| expected_version > source),
    }))
}
pub(super) fn policy(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<ExecutionPolicy, StoreError> {
    let request = read(c, account, id)?.filter(|r| r.active);
    Ok(ExecutionPolicy {
        restored: source_version(c, account, id)?.is_some(),
        mode: request.as_ref().map(|r| r.mode),
        first_attempt_ordinal: request.and_then(|r| r.first_attempt_ordinal),
    })
}
pub(super) fn dispatch_allowed(c: &Connection, op: &Operation) -> Result<bool, StoreError> {
    let policy = policy(c, &op.account_id, &op.id)?;
    if policy.observes_only() {
        return Ok(false);
    }
    if !policy.restored {
        return Ok(true);
    }
    match op.kind.as_str() {
        "mail_change" => {
            let provider = super::accounts::get(c, &op.account_id)?
                .ok_or(StoreError::NotFound)?
                .account
                .provider;
            Ok(provider == "google"
                || matches!(
                    super::imap_transfers::payload(c, &op.account_id, &op.id)?,
                    super::ImapChangePayload::Flags(_)
                ))
        }
        "calendar_change" => Ok(
            super::calendar_changes::payload(c, &op.account_id, &op.id)?.kind
                != super::CalendarWriteKind::Create,
        ),
        _ => Ok(false),
    }
}
pub(super) fn constrain(
    c: &Connection,
    op: &Operation,
    outcome: AttemptOutcome,
) -> Result<AttemptOutcome, StoreError> {
    let policy = policy(c, &op.account_id, &op.id)?;
    let code = match &outcome {
        AttemptOutcome::RetryUnstartedSmtp { .. } if policy.restored => {
            Some("restored_smtp_acceptance_unknown")
        }
        AttemptOutcome::RetryUnstartedImapTransfer { .. } if policy.restored => {
            Some("restored_imap_copy_identity_unknown")
        }
        AttemptOutcome::RepeatableCalendar {
            evidence: super::CalendarRetryEvidence::Missing { .. },
            ..
        } if policy.restored => Some("restored_calendar_create_acceptance_unknown"),
        AttemptOutcome::Repeatable { .. }
        | AttemptOutcome::RepeatableCalendar { .. }
        | AttemptOutcome::RetryUnstartedSmtp { .. }
        | AttemptOutcome::RetryUnstartedImapTransfer { .. }
            if policy.observes_only() =>
        {
            Some("reconciliation_resume_required")
        }
        _ => None,
    };
    Ok(if let Some(code) = code {
        AttemptOutcome::Uncertain { code: code.into() }
    } else {
        outcome
    })
}
impl Store {
    pub(crate) async fn operation_execution_policy(
        &self,
        account: String,
        id: String,
    ) -> Result<ExecutionPolicy, StoreError> {
        self.execute(move |c| policy(c, &account, &id)).await
    }
    pub async fn request_reconciliation(
        &self,
        mut input: ReconcileOperation,
        now: i64,
    ) -> Result<Operation, StoreError> {
        input.request_id = uuid::Uuid::parse_str(&input.request_id)
            .map_err(|_| StoreError::InvalidInput)?
            .to_string();
        if now < 0
            || input.expected_version == 0
            || input.expected_version >= i64::MAX as u64
            || [&input.account_id, &input.operation_id]
                .iter()
                .any(|v| v.is_empty() || v.len() > 128)
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;
            let prior:Option<(String,i64,String)>=tx.query_row("SELECT operation_id,expected_version,mode FROM operation_reconciliation_requests WHERE account_id=?1 AND request_id=?2",params![input.account_id,input.request_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((id,version,mode))=prior {
                if id!=input.operation_id || version!=input.expected_version as i64 || mode!=input.mode.value(){return Err(StoreError::VersionConflict)}
                return operations::get(&tx,&input.account_id,&input.operation_id);
            }
            super::accounts::writable(&tx,&input.account_id)?;
            let op=operations::get(&tx,&input.account_id,&input.operation_id)?;
            if op.version!=input.expected_version || !matches!(op.state.as_str(),"uncertain"|"conflict") || op.disposition.is_some() || op.needs_reconciliation {return Err(StoreError::VersionConflict)}
            if !matches!(op.kind.as_str(),"send"|"mail_change"|"calendar_change"){return Err(StoreError::InvalidInput)}
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM operation_attempts WHERE account_id=?1 AND finished_at_ms IS NULL)",[&input.account_id],|r|r.get::<_,bool>(0))?{return Err(StoreError::Busy)}
            let count:i64=tx.query_row("SELECT count(*) FROM operation_reconciliation_requests WHERE account_id=?1 AND operation_id=?2",params![input.account_id,input.operation_id],|r|r.get(0))?;
            let attempts:i64=tx.query_row("SELECT coalesce(max(ordinal),0) FROM operation_attempts WHERE account_id=?1 AND operation_id=?2",params![input.account_id,input.operation_id],|r|r.get(0))?;
            if count>=1000 || attempts>=1000{return Err(StoreError::ResultTooLarge)}
            let now=now.max(op.updated_at_ms);
            tx.execute("INSERT INTO operation_reconciliation_requests(account_id,request_id,operation_id,expected_version,mode,requested_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",params![input.account_id,input.request_id,input.operation_id,input.expected_version as i64,input.mode.value(),now])?;
            tx.execute("UPDATE operations SET state='uncertain',needs_reconciliation=1,next_attempt_at_ms=NULL,error_code=NULL,version=version+1,updated_at_ms=?3 WHERE account_id=?1 AND id=?2",params![input.account_id,input.operation_id,now])?;
            super::changes::record(&tx,Some(&input.account_id),"operation",Some(&input.operation_id))?;
            let result=operations::get(&tx,&input.account_id,&input.operation_id)?;tx.commit()?;Ok(result)
        }).await
    }
}
