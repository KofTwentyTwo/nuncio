use super::{transfer, ImapCopyProof, ImapTransferMode};
use crate::store::{OperationReceipt, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImapTransferStep {
    Prepared,
    Started,
    Copied,
    DeletingSource,
    ExpungingSource,
    SourceRemoved,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapTransferProgress {
    pub phase: ImapTransferStep,
    pub wire_mode: Option<ImapTransferMode>,
    pub copy: Option<ImapCopyProof>,
}
fn text<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_value(value)
        .map_err(|_| StoreError::InvalidInput)?
        .as_str()
        .map(String::from)
        .ok_or(StoreError::InvalidInput)
}
fn parse<T: serde::de::DeserializeOwned>(value: String) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(value)).map_err(|_| StoreError::KeyOrCorrupt)
}
pub(super) fn progress(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<ImapTransferProgress, StoreError> {
    let intent = transfer(c, account, id)?;
    let row:Option<(String,String,Option<String>)>=c.query_row("SELECT phase,wire_mode,copy_json FROM imap_transfer_progress WHERE account_id=?1 AND operation_id=?2",params![account,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((phase, mode, copy)) = row else {
        return Ok(ImapTransferProgress {
            phase: ImapTransferStep::Prepared,
            wire_mode: None,
            copy: None,
        });
    };
    let copy = copy
        .map(|v| serde_json::from_str::<ImapCopyProof>(&v).map_err(|_| StoreError::KeyOrCorrupt))
        .transpose()?;
    if copy.as_ref().is_some_and(|copy| !intent.accepts(copy)) {
        return Err(StoreError::KeyOrCorrupt);
    }
    Ok(ImapTransferProgress {
        phase: parse(phase)?,
        wire_mode: Some(parse(mode)?),
        copy,
    })
}
fn active(
    c: &Connection,
    account: &str,
    id: &str,
    ordinal: u32,
    now: i64,
) -> Result<i64, StoreError> {
    if now < 0 || ordinal == 0 {
        return Err(StoreError::InvalidInput);
    }
    let op = crate::store::operations::get(c, account, id)?;
    let (kind,finished):(String,Option<i64>)=c.query_row("SELECT kind,finished_at_ms FROM operation_attempts WHERE account_id=?1 AND operation_id=?2 AND ordinal=?3",params![account,id,ordinal],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(StoreError::NotFound)?;
    if op.kind != "mail_change"
        || op.disposition.is_some()
        || finished.is_some()
        || !matches!(
            (kind.as_str(), op.state.as_str()),
            ("dispatch", "running") | ("reconcile", "uncertain")
        )
    {
        return Err(StoreError::VersionConflict);
    }
    Ok(now.max(op.updated_at_ms))
}
fn touch(c: &Connection, account: &str, id: &str, now: i64) -> Result<(), StoreError> {
    c.execute(
        "UPDATE operations SET version=version+1,updated_at_ms=?3 WHERE account_id=?1 AND id=?2",
        params![account, id, now],
    )?;
    crate::store::changes::record(c, Some(account), "operation", Some(id))?;
    Ok(())
}
impl Store {
    pub async fn imap_transfer_progress(
        &self,
        account: String,
        id: String,
    ) -> Result<ImapTransferProgress, StoreError> {
        self.execute(move |c| progress(c, &account, &id)).await
    }
    /// Commit before COPY or MOVE can put bytes on the wire. A started transfer
    /// without a durable copy receipt is never permission to dispatch again.
    pub async fn start_imap_transfer(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        mode: ImapTransferMode,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;let now=active(&tx,&account,&id,ordinal,now)?;
            let intent=transfer(&tx,&account,&id)?;
            if intent.mode==ImapTransferMode::Copy && mode!=ImapTransferMode::Copy {return Err(StoreError::InvalidInput);}
            if progress(&tx,&account,&id)?.phase!=ImapTransferStep::Prepared {return Err(StoreError::VersionConflict);}
            tx.execute("INSERT INTO imap_transfer_progress(account_id,operation_id,phase,wire_mode) VALUES(?1,?2,'started',?3)",params![account,id,text(&mode)?])?;
            touch(&tx,&account,&id,now)?;tx.commit()?;Ok(())
        }).await
    }
    /// The COPYUID proof is recorded before separate source deletion or expunge.
    pub async fn record_imap_copy(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        proof: ImapCopyProof,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;let now=active(&tx,&account,&id,ordinal,now)?;
            if proof.source==proof.destination || !transfer(&tx,&account,&id)?.accepts(&proof) {return Err(StoreError::InvalidInput);}
            let current=progress(&tx,&account,&id)?;
            if let Some(prior)=current.copy {return if prior==proof {Ok(())} else {Err(StoreError::VersionConflict)};}
            if current.phase!=ImapTransferStep::Started {return Err(StoreError::VersionConflict);}
            tx.execute("UPDATE imap_transfer_progress SET phase='copied',copy_json=?3 WHERE account_id=?1 AND operation_id=?2",params![account,id,serde_json::to_string(&proof).map_err(|_|StoreError::InvalidInput)?])?;
            crate::store::operation_attempts::append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"imap_copy".into(),source:"acknowledgement".into(),provider_id:Some(proof.destination.provider_id().map_err(|_|StoreError::InvalidInput)?),etag:None},now)?;
            touch(&tx,&account,&id,now)?;tx.commit()?;Ok(())
        }).await
    }
    pub async fn advance_imap_transfer(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        step: ImapTransferStep,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;let now=active(&tx,&account,&id,ordinal,now)?;
            if transfer(&tx,&account,&id)?.mode!=ImapTransferMode::Move {return Err(StoreError::InvalidInput);}
            let prior=progress(&tx,&account,&id)?;
            if prior.copy.is_none() {return Err(StoreError::VersionConflict);}
            use ImapTransferStep::*;
            if !matches!((prior.phase,step),(Copied,DeletingSource|SourceRemoved)|(DeletingSource,ExpungingSource|SourceRemoved)|(ExpungingSource,SourceRemoved)) {
                return if prior.phase==step && matches!(step,DeletingSource|ExpungingSource|SourceRemoved) {Ok(())} else {Err(StoreError::VersionConflict)};
            }
            tx.execute("UPDATE imap_transfer_progress SET phase=?3 WHERE account_id=?1 AND operation_id=?2",params![account,id,text(&step)?])?;
            touch(&tx,&account,&id,now)?;tx.commit()?;Ok(())
        }).await
    }
}
