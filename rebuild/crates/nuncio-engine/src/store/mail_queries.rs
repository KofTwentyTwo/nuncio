use super::{
    MailAttachment, MailCollection, MailCoverage, MailDetail, MailPage, MailQuery, MailSummary,
    Store, StoreError, StoredBlob,
};
use crate::domain::mail::Header;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    revision: u64,
    fingerprint: String,
    date: i64,
    id: String,
}
impl Store {
    pub async fn query_mail(&self, mut query: MailQuery) -> Result<MailPage, StoreError> {
        if !(1..=100).contains(&query.page_size)
            || query.account_id.len() > 128
            || query.collection_id.as_ref().is_some_and(|s| s.len() > 128)
            || query.query.as_ref().is_some_and(|s| s.len() > 512)
            || query.page_token.as_ref().is_some_and(|s| s.len() > 2048)
        {
            return Err(StoreError::InvalidInput);
        }
        let token = query.page_token.take();
        let fingerprint = hex::encode(Sha256::digest(
            serde_json::to_vec(&query).map_err(|_| StoreError::InvalidInput)?,
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
        let expression = query
            .query
            .as_ref()
            .map(|q| search_expression(q))
            .transpose()?;
        self.execute(move|c|{
            account_exists(c,&query.account_id)?;
            let revision=revision(c)?;
            if cursor.as_ref().is_some_and(|p|p.revision!=revision){return Err(StoreError::RefreshRequired)}
            let mut statement=c.prepare("SELECT m.id,m.account_id,m.provider_id,m.thread_id,m.history_id,m.internal_date_ms,m.subject,m.body_availability FROM messages m WHERE m.account_id=?1 AND (?2 IS NULL OR EXISTS(SELECT 1 FROM memberships mm WHERE mm.account_id=m.account_id AND mm.message_id=m.id AND mm.collection_id=?2)) AND (?3 IS NULL OR m.id IN (SELECT message_id FROM message_search WHERE account_id=?1 AND message_search MATCH ?3)) AND (?4 IS NULL OR coalesce(m.internal_date_ms,0)<?4 OR (coalesce(m.internal_date_ms,0)=?4 AND m.id>?5)) ORDER BY coalesce(m.internal_date_ms,0) DESC,m.id LIMIT ?6")?;
            let mut items=statement.query_map(params![query.account_id,query.collection_id,expression,cursor.as_ref().map(|p|p.date),cursor.as_ref().map(|p|&p.id),query.page_size+1],summary)?.collect::<Result<Vec<_>,_>>()?;
            let more=items.len()>query.page_size as usize;
            items.truncate(query.page_size as usize);
            for item in &mut items {item.collections=collections(c,&query.account_id,Some(&item.id))?;}
            let next_page_token=if more {
                let last=items.last().ok_or(StoreError::KeyOrCorrupt)?;
                Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor{revision,fingerprint,date:last.internal_date_ms.unwrap_or(0),id:last.id.clone()}).map_err(|_|StoreError::InvalidInput)?))
            }else{None};
            Ok(MailPage{revision,items,next_page_token,coverage:coverage(c,&query.account_id)?})
        }).await
    }
    pub async fn get_mail(&self, account: String, id: String) -> Result<MailDetail, StoreError> {
        self.execute(move|c|{
            let mut message=c.query_row("SELECT id,account_id,provider_id,thread_id,history_id,internal_date_ms,subject,body_availability FROM messages WHERE account_id=?1 AND id=?2",params![account,id],summary).optional()?.ok_or(StoreError::NotFound)?;
            message.collections=collections(c,&account,Some(&id))?;
            let mut headers=c.prepare("SELECT name,value FROM message_headers WHERE account_id=?1 AND message_id=?2 ORDER BY ordinal")?;
            let headers=headers.query_map(params![account,id],|r|Ok(Header{name:r.get(0)?,value:r.get(1)?}))?.collect::<Result<Vec<_>,_>>()?;
            let mut attachments=c.prepare("SELECT a.id,a.part_index,a.filename,a.mime_type,a.content_id,b.byte_length,b.sha256 FROM attachments a JOIN blobs b ON b.account_id=a.account_id AND b.id=a.blob_id WHERE a.account_id=?1 AND a.message_id=?2 ORDER BY a.part_index")?;
            let attachments=attachments.query_map(params![account,id],|r|Ok(MailAttachment{id:r.get(0)?,part_index:r.get(1)?,filename:r.get(2)?,mime_type:r.get(3)?,content_id:r.get(4)?,byte_length:u64::from(r.get::<_,u32>(5)?),sha256:r.get(6)?}))?.collect::<Result<Vec<_>,_>>()?;
            let provider_json=c.query_row("SELECT provider_json FROM messages WHERE account_id=?1 AND id=?2",params![account,id],|r|r.get(0))?;
            Ok(MailDetail{revision:revision(c)?,message,headers,attachments,provider_json})
        }).await
    }
    pub async fn mail_collections(
        &self,
        account: String,
    ) -> Result<(u64, Vec<MailCollection>), StoreError> {
        self.execute(move |c| {
            account_exists(c, &account)?;
            Ok((revision(c)?, collections(c, &account, None)?))
        })
        .await
    }
    /// Resolve through message ownership before a stream captures an immutable blob.
    pub async fn mail_blob(
        &self,
        account: String,
        id: String,
        kind: String,
        attachment: Option<String>,
    ) -> Result<StoredBlob, StoreError> {
        if !matches!(kind.as_str(), "raw" | "text" | "html" | "attachment")
            || (kind == "attachment") != attachment.is_some()
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let sql=if kind=="attachment" {
                "SELECT b.id,b.byte_length,b.sha256 FROM attachments a JOIN blobs b ON b.account_id=a.account_id AND b.id=a.blob_id WHERE a.account_id=?1 AND a.message_id=?2 AND a.id=?3"
            } else {
                "SELECT b.id,b.byte_length,b.sha256 FROM messages m JOIN blobs b ON b.account_id=m.account_id AND b.id=CASE ?3 WHEN 'raw' THEN m.raw_blob_id WHEN 'text' THEN m.text_blob_id WHEN 'html' THEN m.html_blob_id END WHERE m.account_id=?1 AND m.id=?2"
            };
            c.query_row(sql,params![account,id,attachment.as_deref().unwrap_or(&kind)],|r|Ok(StoredBlob{id:r.get(0)?,byte_length:u64::from(r.get::<_,u32>(1)?),sha256:r.get(2)?})).optional()?.ok_or(StoreError::NotFound)
        }).await
    }
    pub async fn mail_coverage(&self, account: String) -> Result<MailCoverage, StoreError> {
        self.execute(move |c| {
            account_exists(c, &account)?;
            coverage(c, &account)
        })
        .await
    }
}
fn search_expression(query: &str) -> Result<String, StoreError> {
    let terms: Vec<_> = query.split_whitespace().collect();
    if terms.is_empty() || terms.len() > 64 || query.chars().any(char::is_control) {
        return Err(StoreError::InvalidInput);
    }
    Ok(terms
        .into_iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND "))
}
fn revision(c: &Connection) -> Result<u64, StoreError> {
    let revision: i64 = c.query_row(
        "SELECT revision FROM store_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    u64::try_from(revision).map_err(|_| StoreError::KeyOrCorrupt)
}
fn account_exists(c: &Connection, account: &str) -> Result<(), StoreError> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1)",
        [account],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(StoreError::NotFound);
    }
    Ok(())
}
fn summary(r: &rusqlite::Row<'_>) -> rusqlite::Result<MailSummary> {
    Ok(MailSummary {
        id: r.get(0)?,
        account_id: r.get(1)?,
        provider_id: r.get(2)?,
        thread_id: r.get(3)?,
        history_id: r.get(4)?,
        internal_date_ms: r.get(5)?,
        subject: r.get(6)?,
        body_availability: r.get(7)?,
        collections: vec![],
    })
}
fn collections(
    c: &Connection,
    account: &str,
    message: Option<&str>,
) -> Result<Vec<MailCollection>, StoreError> {
    let mut statement=c.prepare("SELECT c.id,c.provider_id,c.name,c.kind,c.retired FROM collections c WHERE c.account_id=?1 AND (?2 IS NULL OR EXISTS(SELECT 1 FROM memberships m WHERE m.account_id=c.account_id AND m.collection_id=c.id AND m.message_id=?2)) ORDER BY c.provider_id LIMIT 4097")?;
    let items = statement
        .query_map(params![account, message], |r| {
            Ok(MailCollection {
                id: r.get(0)?,
                provider_id: r.get(1)?,
                name: r.get(2)?,
                kind: r.get(3)?,
                retired: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if items.len() > 4096 {
        return Err(StoreError::InvalidInput);
    }
    Ok(items)
}
fn coverage(c: &Connection, account: &str) -> Result<MailCoverage, StoreError> {
    Ok(c.query_row(
        "SELECT synchronized_at_ms,cursor FROM sync_scopes WHERE account_id=?1 AND scope=(SELECT CASE provider WHEN 'imap' THEN 'imap' ELSE 'gmail' END FROM accounts WHERE id=?1)",
        [account],
        |r| {
            Ok(MailCoverage {
                state: "current".into(),
                synchronized_at_ms: Some(r.get(0)?),
                cursor: Some(r.get(1)?),
            })
        },
    )
    .optional()?
    .unwrap_or(MailCoverage {
        state: "unavailable".into(),
        synchronized_at_ms: None,
        cursor: None,
    }))
}
