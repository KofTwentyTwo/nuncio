use super::{operations, Operation, Store, StoreError};
use crate::domain::submission::FrozenMessage;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveOperation {
    pub account_id: String,
    pub operation_id: String,
    pub expected_version: u64,
    pub decision: ResolutionDecision,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResolutionDecision {
    Abandon {
        reason: String,
    },
    ConfirmApplied {
        evidence: ConfirmationEvidence,
    },
    Resend {
        request_id: String,
        accept_duplicate_risk: bool,
        reason: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmationEvidence {
    pub provider_id: String,
    pub message_id: Option<String>,
    pub etag: Option<String>,
    pub observed_at_ms: i64,
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp_acceptance_note: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct OperationResolution {
    pub decision: String,
    pub decided_at_ms: i64,
    pub expected_version: u64,
    pub evidence: Value,
    pub replacement_id: Option<String>,
}
impl ResolveOperation {
    fn canonical(&self) -> Result<Value, StoreError> {
        if self.account_id.is_empty()
            || self.account_id.len() > 128
            || self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || self.expected_version == 0
            || self.expected_version > i64::MAX as u64
        {
            return Err(StoreError::InvalidInput);
        }
        match &self.decision {
            ResolutionDecision::Abandon { reason } => text(reason, 4096)?,
            ResolutionDecision::ConfirmApplied { evidence } => {
                text(&evidence.provider_id, 2048)?;
                text(&evidence.note, 4096)?;
                if let Some(note) = &evidence.smtp_acceptance_note {
                    text(note, 4096)?;
                }
                for value in [&evidence.message_id, &evidence.etag].into_iter().flatten() {
                    text(value, 2048)?;
                }
                if evidence.observed_at_ms < 0 {
                    return Err(StoreError::InvalidInput);
                }
            }
            ResolutionDecision::Resend {
                request_id,
                accept_duplicate_risk,
                reason,
            } => {
                if !accept_duplicate_risk {
                    return Err(StoreError::InvalidInput);
                }
                let canonical = uuid::Uuid::parse_str(request_id)
                    .map_err(|_| StoreError::InvalidInput)?
                    .to_string();
                if canonical != *request_id {
                    return Err(StoreError::InvalidInput);
                }
                text(reason, 4096)?;
            }
        }
        serde_json::to_value(self).map_err(|_| StoreError::InvalidInput)
    }
}
impl Store {
    pub async fn find_resolution(
        &self,
        input: ResolveOperation,
    ) -> Result<Option<Operation>, StoreError> {
        let canonical = input.canonical()?;
        self.execute(move |c| {
            let op = operations::get(c, &input.account_id, &input.operation_id)?;
            if resolved(&op, &canonical)? {
                return Ok(Some(op));
            }
            admissible(c, &op, input.expected_version)?;
            Ok(None)
        })
        .await
    }
    pub async fn resolve_operation(
        &self,
        input: ResolveOperation,
        replacement: Option<FrozenMessage>,
        now: i64,
    ) -> Result<Operation, StoreError> {
        self.resolve_operation_guarded(input, replacement, now, None)
            .await
    }
    pub(crate) async fn resolve_operation_guarded(
        &self,
        input: ResolveOperation,
        replacement: Option<FrozenMessage>,
        now: i64,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<Operation, StoreError> {
        let canonical = input.canonical()?;
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move |c| {
            let _permit=permit;
            let tx=c.transaction()?;
            let account=&input.account_id; let id=&input.operation_id;
            let op=operations::get(&tx,account,id)?;
            if resolved(&op,&canonical)? { return Ok(op); }
            admissible(&tx,&op,input.expected_version)?;
            let now=now.max(op.updated_at_ms);
            let (decision,state,disposition,replacement_id)=match input.decision {
                ResolutionDecision::Abandon{..} => {
                    if replacement.is_some() { return Err(StoreError::InvalidInput); }
                    ("abandon",op.state.as_str(),"abandoned",None)
                },
                ResolutionDecision::ConfirmApplied{ref evidence} => {
                    if replacement.is_some() || op.state!="uncertain" || evidence.observed_at_ms<op.created_at_ms || evidence.observed_at_ms>now { return Err(StoreError::InvalidInput); }
                    // A human's observation is retained as such; it never becomes
                    // a fabricated provider attempt or transport acknowledgement.
                    if op.kind!="send" { return Err(StoreError::InvalidInput); }
                    let message_id:String=tx.query_row("SELECT message_id FROM send_payloads WHERE account_id=?1 AND operation_id=?2",params![account,id],|r|r.get(0))?;
                    if evidence.message_id.as_deref()!=Some(&message_id) { return Err(StoreError::InvalidInput); }
                    confirm_provider(&tx,&op,evidence)?;
                    ("confirm_applied","applied","manual_confirmed",None)
                },
                ResolutionDecision::Resend{ref request_id,..} => {
                    if op.kind!="send" || request_id==&op.request_id { return Err(StoreError::InvalidInput); }
                    let frozen=replacement.ok_or(StoreError::InvalidInput)?;
                    let replacement=insert_resend(&tx,&op,request_id,&canonical,frozen,now)?;
                    ("resend",op.state.as_str(),"resend_requested",Some(replacement))
                }
            };
            tx.execute("INSERT INTO operation_resolutions(account_id,operation_id,sequence,decided_at_ms,decision,evidence_json,replacement_id) VALUES (?1,?2,1,?3,?4,?5,?6)",params![account,id,now,decision,canonical.to_string(),replacement_id])?;
            tx.execute("UPDATE operations SET state=?3,disposition=?4,needs_reconciliation=0,next_attempt_at_ms=NULL,error_code=CASE WHEN ?3='applied' THEN NULL ELSE error_code END,version=version+1,updated_at_ms=?5 WHERE account_id=?1 AND id=?2",params![account,id,state,disposition,now])?;
            super::changes::record(&tx,Some(account),"operation",Some(id))?;
            let result=operations::get(&tx,account,id)?;
            tx.commit()?;
            Ok(result)
        }).await
    }
}
fn confirm_provider(
    c: &Connection,
    op: &Operation,
    evidence: &ConfirmationEvidence,
) -> Result<(), StoreError> {
    let provider = super::accounts::get(c, &op.account_id)?
        .ok_or(StoreError::NotFound)?
        .account
        .provider;
    match provider.as_str() {
        "google" if evidence.smtp_acceptance_note.is_none() => Ok(()),
        "imap" if evidence.smtp_acceptance_note.is_some() && evidence.etag.is_none() => {
            let account = op
                .account_id
                .parse()
                .map_err(|_| StoreError::InvalidAccount)?;
            let placement = crate::domain::imap::ImapPlacement::from_provider_id(
                account,
                &evidence.provider_id,
            )
            .map_err(|_| StoreError::InvalidInput)?;
            if placement
                .provider_id()
                .map_err(|_| StoreError::InvalidInput)?
                != evidence.provider_id
            {
                return Err(StoreError::InvalidInput);
            }
            let intent = super::smtp::intent(c, &op.account_id, &op.id)?;
            let progress = super::smtp::progress(c, &op.account_id, &op.id)?;
            if !intent.accepts(placement)
                || progress.step == super::SmtpStep::Prepared
                || progress
                    .sent_floor
                    .is_some_and(|floor| placement.uid < floor)
                || progress.placement.is_some_and(|known| known != placement)
            {
                return Err(StoreError::InvalidInput);
            }
            Ok(())
        }
        _ => Err(StoreError::InvalidInput),
    }
}
fn text(value: &str, max: usize) -> Result<(), StoreError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}
fn resolved(op: &Operation, canonical: &Value) -> Result<bool, StoreError> {
    if let Some(resolution) = op.resolutions.first() {
        if resolution.evidence == *canonical {
            return Ok(true);
        }
        return Err(StoreError::VersionConflict);
    }
    Ok(false)
}
fn admissible(c: &Connection, op: &Operation, version: u64) -> Result<(), StoreError> {
    if op.version != version
        || op.disposition.is_some()
        || !matches!(op.state.as_str(), "uncertain" | "conflict")
    {
        return Err(StoreError::VersionConflict);
    }
    if c.query_row("SELECT EXISTS(SELECT 1 FROM operation_attempts WHERE account_id=?1 AND operation_id=?2 AND finished_at_ms IS NULL)",params![op.account_id,op.id],|r|r.get::<_,bool>(0))? { return Err(StoreError::Busy); }
    Ok(())
}
fn insert_resend(
    c: &Connection,
    op: &Operation,
    request_id: &str,
    canonical: &Value,
    frozen: FrozenMessage,
    now: i64,
) -> Result<String, StoreError> {
    let (draft,version,sender,recipients,message_id,thread,sent):(String,i64,String,String,String,Option<String>,Option<String>)=c.query_row("SELECT draft_id,draft_version,sender,recipients_json,message_id,thread_id,sent_blob_id FROM send_payloads WHERE account_id=?1 AND operation_id=?2",params![op.account_id,op.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?;
    let recipients: Vec<String> =
        serde_json::from_str(&recipients).map_err(|_| StoreError::KeyOrCorrupt)?;
    if frozen.message_id == message_id
        || frozen.message_id.is_empty()
        || frozen.recipients != recipients
        || frozen.thread_id != thread
        || frozen.sent_copy.is_some() != sent.is_some()
    {
        return Err(StoreError::InvalidInput);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let canonical = canonical.to_string();
    let fingerprint = hex::encode(Sha256::digest(canonical.as_bytes()));
    let desired = json!({"draft_id":draft,"draft_version":version,"submission":"send","resend_of":op.id,"sender":sender,"recipients":frozen.recipients,"message_id":frozen.message_id,"thread_id":frozen.thread_id});
    let inserted=c.execute("INSERT INTO operations(account_id,id,request_id,kind,resource_id,fingerprint,request_json,desired_json,state,created_at_ms,updated_at_ms) VALUES (?1,?2,?3,'send',?4,?5,?6,?7,'queued',?8,?8) ON CONFLICT(account_id,request_id) DO NOTHING",params![op.account_id,id,request_id,draft,fingerprint,canonical,desired.to_string(),now])?;
    if inserted != 1 {
        return Err(StoreError::VersionConflict);
    }
    let wire = super::blobs::insert(c, &op.account_id, &frozen.wire)?;
    let sent = frozen
        .sent_copy
        .as_ref()
        .map(|b| super::blobs::insert(c, &op.account_id, b))
        .transpose()?;
    c.execute("INSERT INTO send_payloads(account_id,operation_id,draft_id,draft_version,sender,recipients_json,message_id,thread_id,wire_blob_id,sent_blob_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![op.account_id,id,draft,version,sender,serde_json::to_string(&frozen.recipients).map_err(|_|StoreError::InvalidInput)?,frozen.message_id,frozen.thread_id,wire.id,sent.map(|b|b.id)])?;
    if super::accounts::get(c, &op.account_id)?
        .ok_or(StoreError::NotFound)?
        .account
        .provider
        == "imap"
    {
        super::smtp::capture(
            c,
            &op.account_id,
            &id,
            frozen.sent_copy.as_deref().unwrap_or(&frozen.wire),
        )?;
    }
    super::changes::record(c, Some(&op.account_id), "operation", Some(&id))?;
    Ok(id)
}
pub(super) fn read(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<Vec<OperationResolution>, StoreError> {
    let mut stmt=c.prepare("SELECT decided_at_ms,decision,evidence_json,replacement_id FROM operation_resolutions WHERE account_id=?1 AND operation_id=?2 ORDER BY sequence LIMIT 2")?;
    let rows = stmt.query_map(params![account, id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut resolutions = Vec::new();
    for row in rows {
        let (decided_at_ms, decision, evidence, replacement_id) = row?;
        let evidence: Value =
            serde_json::from_str(&evidence).map_err(|_| StoreError::KeyOrCorrupt)?;
        let expected_version = evidence
            .get("expected_version")
            .and_then(Value::as_u64)
            .ok_or(StoreError::KeyOrCorrupt)?;
        resolutions.push(OperationResolution {
            decided_at_ms,
            decision,
            evidence,
            expected_version,
            replacement_id,
        });
    }
    if resolutions.len() > 1 {
        return Err(StoreError::KeyOrCorrupt);
    }
    Ok(resolutions)
}
