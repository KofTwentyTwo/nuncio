use super::*;
use crate::store::OperationReceipt;
impl Store {
    pub async fn record_server_sent(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        evidence: ServerSentEvidence,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;let now=active(&tx,&account,&id,ordinal,now)?;
            let intent=intent(&tx,&account,&id)?;let prior=progress(&tx,&account,&id)?;
            let placement=evidence.placement;
            if intent.sent_policy!=SentPolicy::Server || !intent.accepts(placement) || prior.sent_floor.is_none_or(|floor|placement.uid<floor) || intent.sent_fingerprint.as_ref().is_none_or(|expected|!expected.matches(&evidence.fingerprint)) {return Err(StoreError::InvalidInput);}
            if prior.step==SmtpStep::Copied && prior.placement==Some(placement) {return Ok(());}
            if prior.step!=SmtpStep::Accepted {return Err(StoreError::VersionConflict);}
            tx.execute("UPDATE smtp_submissions SET step='copied',placement_json=?3 WHERE account_id=?1 AND operation_id=?2",params![account,id,serde_json::to_string(&placement).map_err(|_|StoreError::InvalidInput)?])?;
            crate::store::operation_attempts::append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"server_sent_observed".into(),source:"positive_read".into(),provider_id:Some(placement.provider_id().map_err(|_|StoreError::InvalidInput)?),etag:None},now)?;
            touch(&tx,&account,&id,now)?;tx.commit()?;Ok(())
        }).await
    }
    /// Only a complete negative APPEND response authorizes another copy attempt.
    /// SMTP acceptance remains permanent and is never submitted again here.
    pub async fn record_sent_rejection(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;let now=active(&tx,&account,&id,ordinal,now)?;
            let intent=intent(&tx,&account,&id)?;
            if intent.sent_policy!=SentPolicy::ClientAppend || progress(&tx,&account,&id)?.step!=SmtpStep::Appending {return Err(StoreError::VersionConflict);}
            tx.execute("UPDATE smtp_submissions SET step='accepted' WHERE account_id=?1 AND operation_id=?2",params![account,id])?;
            crate::store::operation_attempts::append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"imap_append_rejected".into(),source:"acknowledgement".into(),provider_id:Some(intent.mailbox_id.to_string()),etag:None},now)?;
            touch(&tx,&account,&id,now)?;tx.commit()?;Ok(())
        }).await
    }
    /// Must commit before any DATA content, including the terminator, is written.
    pub async fn start_smtp_data(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        observed: crate::domain::imap::ImapMailboxState,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;let now=active(&tx,&account,&id,ordinal,now)?;
            if operations::get(&tx,&account,&id)?.state!="running" || progress(&tx,&account,&id)?.step!=SmtpStep::Prepared {return Err(StoreError::VersionConflict);}
            let intent=intent(&tx,&account,&id)?;
            let observed=observed.validated().map_err(|_|StoreError::InvalidInput)?;
            let floor=observed.uid_next.ok_or(StoreError::InvalidInput)?;
            if observed.name!=intent.mailbox || observed.uid_validity!=Some(intent.uid_validity.get()) || floor<intent.uid_next.get() {return Err(StoreError::VersionConflict);}
            tx.execute("UPDATE smtp_submissions SET step='started',sent_floor=?3 WHERE account_id=?1 AND operation_id=?2",params![account,id,floor])?;
            touch(&tx,&account,&id,now)?;tx.commit()?;Ok(())
        }).await
    }
    /// A failed Sent copy can never reset the accepted SMTP delivery phase.
    pub async fn start_sent_append(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;let now=active(&tx,&account,&id,ordinal,now)?;
            if intent(&tx,&account,&id)?.sent_policy!=SentPolicy::ClientAppend || progress(&tx,&account,&id)?.step!=SmtpStep::Accepted {return Err(StoreError::VersionConflict);}
            tx.execute("UPDATE smtp_submissions SET step='appending' WHERE account_id=?1 AND operation_id=?2",params![account,id])?;
            touch(&tx,&account,&id,now)?;tx.commit()?;Ok(())
        }).await
    }
    pub async fn record_sent_append(
        &self,
        account: String,
        id: String,
        ordinal: u32,
        placement: ImapPlacement,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;let now=active(&tx,&account,&id,ordinal,now)?;
            let intent=intent(&tx,&account,&id)?;let prior=progress(&tx,&account,&id)?;
            if intent.sent_policy!=SentPolicy::ClientAppend || !intent.accepts(placement) || prior.sent_floor.is_some_and(|floor|placement.uid<floor) {return Err(StoreError::InvalidInput);}
            if prior.step==SmtpStep::Copied && prior.placement==Some(placement) {return Ok(());}
            if prior.step!=SmtpStep::Appending {return Err(StoreError::VersionConflict);}
            tx.execute("UPDATE smtp_submissions SET step='copied',placement_json=?3 WHERE account_id=?1 AND operation_id=?2",params![account,id,serde_json::to_string(&placement).map_err(|_|StoreError::InvalidInput)?])?;
            crate::store::operation_attempts::append_receipt(&tx,&account,&id,ordinal,&OperationReceipt{kind:"imap_append".into(),source:"acknowledgement".into(),provider_id:Some(placement.provider_id().map_err(|_|StoreError::InvalidInput)?),etag:None},now)?;
            touch(&tx,&account,&id,now)?;tx.commit()?;Ok(())
        }).await
    }
}
