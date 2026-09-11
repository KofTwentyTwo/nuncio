use super::{Store, StoreError};
use rusqlite::{params, Connection};
use serde::Serialize;
pub(super) const RETAINED_CHANGES: i64 = 10_000;

#[derive(Clone, Debug, Serialize)]
pub struct ChangeEvent {
    pub revision: u64,
    pub kind: String,
    pub account_id: Option<String>,
    pub resource_id: Option<String>,
}
pub struct ChangePage {
    pub current_revision: u64,
    pub changes: Vec<ChangeEvent>,
    pub has_more: bool,
}

pub(super) fn record(
    c: &Connection,
    account: Option<&str>,
    kind: &str,
    resource: Option<&str>,
) -> Result<(), StoreError> {
    c.execute(
        "UPDATE store_meta SET revision=revision+1 WHERE singleton=1",
        [],
    )?;
    c.execute("INSERT INTO change_log(revision,kind,account_id,resource_id) SELECT revision,?1,?2,?3 FROM store_meta WHERE singleton=1",params![kind,account,resource])?;
    c.execute("DELETE FROM change_log WHERE revision<=(SELECT revision-?1 FROM store_meta WHERE singleton=1)",[RETAINED_CHANGES])?;
    Ok(())
}
impl Store {
    pub async fn watch_changes(
        &self,
        after: u64,
    ) -> Result<crate::changes::ChangeReader, StoreError> {
        crate::changes::ChangeReader::open(self.clone(), after).await
    }
    pub async fn changes_after(&self, after: u64, size: u32) -> Result<ChangePage, StoreError> {
        if !(1..=1000).contains(&size) {
            return Err(StoreError::InvalidInput);
        }
        let after = i64::try_from(after).map_err(|_| StoreError::ChangeHistoryExpired)?;
        self.execute(move|c|{
            let (current,oldest):(i64,Option<i64>)=c.query_row("SELECT revision,(SELECT min(revision) FROM change_log) FROM store_meta WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if after>current || after<oldest.unwrap_or(current+1)-1 {return Err(StoreError::ChangeHistoryExpired);}
            let mut stmt=c.prepare("SELECT revision,kind,account_id,resource_id FROM change_log WHERE revision>?1 ORDER BY revision LIMIT ?2")?;
            let mut rows=stmt.query(params![after,size+1])?;let mut changes=Vec::new();let mut previous=after;
            while let Some(r)=rows.next()?{
                let revision:i64=r.get(0)?;
                if revision!=previous+1{return Err(StoreError::ChangeHistoryExpired);}
                changes.push(ChangeEvent{revision:u64::try_from(revision).map_err(|_|StoreError::KeyOrCorrupt)?,kind:r.get(1)?,account_id:r.get(2)?,resource_id:r.get(3)?});previous=revision;
            }
            let has_more=changes.len()>size as usize;changes.truncate(size as usize);
            Ok(ChangePage{current_revision:u64::try_from(current).map_err(|_|StoreError::KeyOrCorrupt)?,changes,has_more})
        }).await
    }
}
