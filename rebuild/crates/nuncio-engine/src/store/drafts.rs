use super::{changes, Store, StoreError};
use crate::domain::drafts::DraftContent;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct SaveDraft {
    pub account_id: String,
    pub id: Option<String>,
    pub expected_version: Option<u64>,
    pub content: DraftContent,
}
#[derive(Clone, Serialize)]
pub struct Draft {
    pub id: String,
    pub account_id: String,
    pub version: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub content: DraftContent,
    pub attachments: Vec<DraftAttachment>,
    pub context: Option<crate::domain::prepare::DraftContext>,
}
#[derive(Clone, Serialize)]
pub struct DraftAttachment {
    pub id: String,
    #[serde(skip)]
    pub blob_id: String,
    pub filename: Option<String>,
    pub mime_type: String,
    pub parameters: std::collections::BTreeMap<String, String>,
    pub content_id: Option<String>,
    pub disposition: String,
    pub byte_length: u64,
    pub sha256: String,
}
#[derive(Serialize)]
pub struct DraftSummary {
    pub id: String,
    pub account_id: String,
    pub version: u64,
    pub subject: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub attachment_count: u64,
}
#[derive(Serialize)]
pub struct DraftPage {
    pub revision: u64,
    pub items: Vec<DraftSummary>,
    pub next_page_token: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    revision: u64,
    fingerprint: String,
    time: i64,
    id: String,
}
impl Store {
    pub async fn save_draft(&self, input: SaveDraft, now: i64) -> Result<Draft, StoreError> {
        input
            .content
            .validate()
            .map_err(|_| StoreError::InvalidInput)?;
        if now < 0
            || input.id.is_some() != input.expected_version.is_some()
            || input
                .id
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 128)
            || input
                .expected_version
                .is_some_and(|v| v == 0 || v >= i64::MAX as u64)
        {
            return Err(StoreError::InvalidInput);
        }
        let body = serde_json::to_string(&input.content).map_err(|_| StoreError::InvalidInput)?;
        if body.len() > 8 * 1024 * 1024 {
            return Err(StoreError::ResultTooLarge);
        }
        self.execute(move|c|{
            let tx=c.transaction()?; super::accounts::writable(&tx,&input.account_id)?;
            let id=if let Some(id)=input.id {
                let old=get(&tx,&input.account_id,&id)?;
                if Some(old.version)!=input.expected_version { return Err(StoreError::VersionConflict); }
                tx.execute("UPDATE drafts SET version=version+1,subject=?3,content_json=?4,updated_at_ms=?5 WHERE account_id=?1 AND id=?2",params![input.account_id,id,input.content.subject,body,now.max(old.updated_at_ms)])?;
                id
            } else {
                let id=uuid::Uuid::new_v4().to_string();
                tx.execute("INSERT INTO drafts(account_id,id,version,subject,content_json,created_at_ms,updated_at_ms) VALUES (?1,?2,1,?3,?4,?5,?5)",params![input.account_id,id,input.content.subject,body,now])?;
                id
            };
            changes::record(&tx,Some(&input.account_id),"draft",Some(&id))?;
            let draft=get(&tx,&input.account_id,&id)?;
            tx.commit()?;Ok(draft)
        }).await
    }
    pub async fn get_draft(&self, account: String, id: String) -> Result<Draft, StoreError> {
        self.execute(move |c| get(c, &account, &id)).await
    }
    pub async fn delete_draft(
        &self,
        account: String,
        id: String,
        expected_version: u64,
    ) -> Result<(), StoreError> {
        self.execute(move |c| {
            let tx = c.transaction()?;
            super::accounts::writable(&tx, &account)?;
            let draft = get(&tx, &account, &id)?;
            if draft.version != expected_version {
                return Err(StoreError::VersionConflict);
            }
            tx.execute(
                "DELETE FROM drafts WHERE account_id=?1 AND id=?2",
                params![account, id],
            )?;
            changes::record(&tx, Some(&account), "draft", Some(&id))?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn query_drafts(
        &self,
        account: String,
        page_size: u32,
        token: Option<String>,
    ) -> Result<DraftPage, StoreError> {
        if !(1..=100).contains(&page_size)
            || account.len() > 128
            || token.as_ref().is_some_and(|t| t.len() > 2048)
        {
            return Err(StoreError::InvalidInput);
        }
        let fingerprint = hex::encode(Sha256::digest(
            serde_json::to_vec(&("drafts", &account, page_size))
                .map_err(|_| StoreError::InvalidInput)?,
        ));
        let cursor: Option<Cursor> = token
            .map(|t| {
                URL_SAFE_NO_PAD
                    .decode(t)
                    .map_err(|_| StoreError::InvalidInput)
                    .and_then(|b| serde_json::from_slice(&b).map_err(|_| StoreError::InvalidInput))
            })
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|c| c.fingerprint != fingerprint || c.id.len() > 128)
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            account_exists(c,&account)?;
            let revision=c.query_row("SELECT revision FROM store_meta WHERE singleton=1",[],|r|number(r,0))?;
            if cursor.as_ref().is_some_and(|p|p.revision!=revision) { return Err(StoreError::RefreshRequired); }
            let mut stmt=c.prepare("SELECT d.id,d.account_id,d.version,d.subject,d.created_at_ms,d.updated_at_ms,(SELECT count(*) FROM draft_attachments a WHERE a.account_id=d.account_id AND a.draft_id=d.id) FROM drafts d WHERE d.account_id=?1 AND (?2 IS NULL OR d.updated_at_ms<?2 OR (d.updated_at_ms=?2 AND d.id>?3)) ORDER BY d.updated_at_ms DESC,d.id LIMIT ?4")?;
            let mut items=stmt.query_map(params![account,cursor.as_ref().map(|p|p.time),cursor.as_ref().map(|p|&p.id),page_size+1],|r|Ok(DraftSummary{id:r.get(0)?,account_id:r.get(1)?,version:number(r,2)?,subject:r.get(3)?,created_at_ms:r.get(4)?,updated_at_ms:r.get(5)?,attachment_count:number(r,6)?}))?.collect::<Result<Vec<_>,_>>()?;
            let more=items.len()>page_size as usize;items.truncate(page_size as usize);
            let next_page_token=if more {let last=items.last().ok_or(StoreError::KeyOrCorrupt)?;Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor{revision,fingerprint,time:last.updated_at_ms,id:last.id.clone()}).map_err(|_|StoreError::KeyOrCorrupt)?))}else{None};
            Ok(DraftPage{revision,items,next_page_token})
        }).await
    }
}
pub(super) fn get(c: &Connection, account: &str, id: &str) -> Result<Draft, StoreError> {
    let (version,created_at_ms,updated_at_ms,body):(u64,i64,i64,String)=c.query_row("SELECT version,created_at_ms,updated_at_ms,content_json FROM drafts WHERE account_id=?1 AND id=?2",params![account,id],|r|Ok((number(r,0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?.ok_or(StoreError::NotFound)?;
    let content: DraftContent =
        serde_json::from_str(&body).map_err(|_| StoreError::KeyOrCorrupt)?;
    content.validate().map_err(|_| StoreError::KeyOrCorrupt)?;
    let context: Option<String> = c.query_row(
        "SELECT context_json FROM drafts WHERE account_id=?1 AND id=?2",
        params![account, id],
        |r| r.get(0),
    )?;
    let context: Option<crate::domain::prepare::DraftContext> = context
        .map(|s| serde_json::from_str(&s).map_err(|_| StoreError::KeyOrCorrupt))
        .transpose()?;
    if let Some(context) = &context {
        context.validate().map_err(|_| StoreError::KeyOrCorrupt)?;
    }
    let mut stmt=c.prepare("SELECT a.id,a.blob_id,a.filename,a.mime_type,a.content_id,a.disposition,b.byte_length,b.sha256,a.parameters_json FROM draft_attachments a JOIN blobs b ON b.account_id=a.account_id AND b.id=a.blob_id WHERE a.account_id=?1 AND a.draft_id=?2 ORDER BY a.position")?;
    let attachments = stmt
        .query_map(params![account, id], |r| {
            Ok(DraftAttachment {
                id: r.get(0)?,
                blob_id: r.get(1)?,
                filename: r.get(2)?,
                mime_type: r.get(3)?,
                content_id: r.get(4)?,
                disposition: r.get(5)?,
                byte_length: number(r, 6)?,
                sha256: r.get(7)?,
                parameters: serde_json::from_str(&r.get::<_, String>(8)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Draft {
        id: id.into(),
        account_id: account.into(),
        version,
        created_at_ms,
        updated_at_ms,
        content,
        attachments,
        context,
    })
}
fn account_exists(c: &Connection, account: &str) -> Result<(), StoreError> {
    if !c.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1)",
        [account],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

fn number(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
