use super::{blobs, StagedMail, StoreError};
use rusqlite::{params, Connection};

/// Install one provider-observed message inside the caller's receipt transaction.
/// This does not assert a complete inventory or advance a scope checkpoint.
pub(super) fn upsert(
    c: &Connection,
    account: &str,
    mut mail: StagedMail,
) -> Result<String, StoreError> {
    if mail.provider_id.is_empty()
        || mail.provider_id.len() > 2048
        || mail.provider_json.len() > 256 * 1024
        || mail.labels.len() > 4096
        || mail
            .labels
            .iter()
            .any(|id| id.is_empty() || id.len() > 2048)
        || !matches!(
            mail.availability.as_str(),
            "available" | "missing" | "unparsed" | "too_large"
        )
        || (mail.availability == "available" && (mail.raw.is_none() || mail.decoded.is_none()))
    {
        return Err(StoreError::InvalidInput);
    }
    let raw = mail
        .raw
        .as_ref()
        .map(|bytes| blobs::insert(c, account, bytes))
        .transpose()?;
    let text = mail
        .decoded
        .as_ref()
        .and_then(|d| d.text.as_ref())
        .map(|text| blobs::insert(c, account, text.as_bytes()))
        .transpose()?;
    let html = mail
        .decoded
        .as_ref()
        .and_then(|d| d.html.as_ref())
        .map(|text| blobs::insert(c, account, text.as_bytes()))
        .transpose()?;
    if let Some(decoded) = &mail.decoded {
        mail.subject = decoded.subject.clone();
        mail.headers = decoded.headers.clone();
    }
    c.execute("INSERT INTO messages(account_id,id,provider_id,thread_id,history_id,internal_date_ms,subject,provider_json,raw_blob_id,text_blob_id,html_blob_id,body_availability) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(account_id,provider_id) DO UPDATE SET thread_id=excluded.thread_id,history_id=excluded.history_id,internal_date_ms=excluded.internal_date_ms,subject=excluded.subject,provider_json=excluded.provider_json,raw_blob_id=excluded.raw_blob_id,text_blob_id=excluded.text_blob_id,html_blob_id=excluded.html_blob_id,body_availability=excluded.body_availability",params![account,uuid::Uuid::new_v4().to_string(),mail.provider_id,mail.thread_id,mail.history_id,mail.internal_date_ms,mail.subject,mail.provider_json,raw.map(|b|b.id),text.map(|b|b.id),html.map(|b|b.id),mail.availability])?;
    let id: String = c.query_row(
        "SELECT id FROM messages WHERE account_id=?1 AND provider_id=?2",
        params![account, mail.provider_id],
        |r| r.get(0),
    )?;
    c.execute(
        "DELETE FROM message_headers WHERE account_id=?1 AND message_id=?2",
        params![account, id],
    )?;
    for (ordinal, header) in mail.headers.into_iter().enumerate() {
        c.execute(
            "INSERT INTO message_headers VALUES(?1,?2,?3,?4,?5)",
            params![
                account,
                id,
                i64::try_from(ordinal).map_err(|_| StoreError::InvalidInput)?,
                header.name,
                header.value
            ],
        )?;
    }
    c.execute(
        "DELETE FROM memberships WHERE account_id=?1 AND message_id=?2",
        params![account, id],
    )?;
    for label in &mail.labels {
        c.execute("INSERT INTO memberships(account_id,message_id,collection_id) SELECT account_id,?2,id FROM collections WHERE account_id=?1 AND provider_id=?3 AND retired=0",params![account,id,label])?;
    }
    if let Some(decoded) = mail.decoded.as_ref() {
        for attachment in &decoded.attachments {
            let blob = blobs::insert(c, account, &attachment.bytes)?;
            c.execute("INSERT INTO attachments(account_id,id,message_id,part_index,filename,mime_type,content_id,blob_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(account_id,message_id,part_index) DO UPDATE SET filename=excluded.filename,mime_type=excluded.mime_type,content_id=excluded.content_id,blob_id=excluded.blob_id",params![account,uuid::Uuid::new_v4().to_string(),id,attachment.part_index,attachment.filename,attachment.mime_type,attachment.content_id,blob.id])?;
        }
    }
    let parts = mail
        .decoded
        .as_ref()
        .map(|d| {
            d.attachments
                .iter()
                .map(|a| a.part_index)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    c.execute("DELETE FROM attachments WHERE account_id=?1 AND message_id=?2 AND part_index NOT IN (SELECT value FROM json_each(?3))",params![account,id,serde_json::to_string(&parts).map_err(|_|StoreError::InvalidInput)?])?;
    c.execute(
        "DELETE FROM message_search WHERE account_id=?1 AND message_id=?2",
        params![account, id],
    )?;
    c.execute(
        "INSERT INTO message_search(account_id,message_id,subject,body) VALUES(?1,?2,?3,?4)",
        params![
            account,
            id,
            mail.subject,
            mail.decoded.as_ref().map(|d| &d.search_text)
        ],
    )?;
    Ok(id)
}
