use crate::{
    domain::mail::{decode_mime, Header},
    mail::{MailError, MailSync},
    store::{StagedMail, SyncRun},
};
use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use tokio::sync::watch;

const ENVELOPE_LIMIT: usize = 256 * 1024;
const PAGE_LIMIT: usize = 2 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    id: String,
    thread_id: Option<String>,
    history_id: Option<String>,
    internal_date: Option<String>,
    #[serde(default)]
    label_ids: Vec<String>,
    payload: Option<Payload>,
}
#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    headers: Vec<Header>,
}

pub async fn sync(
    service: &MailSync,
    run: &SyncRun,
    cursor: Option<String>,
    fetch: Option<String>,
    stop: watch::Receiver<bool>,
) -> Result<(), MailError> {
    if let Some(id) = fetch {
        let message = service.store.get_mail(run.account_id.clone(), id).await?;
        service
            .store
            .begin_sync_run(run.account_id.clone(), run.id.clone(), None)
            .await?;
        ingest(service, run, &message.message.provider_id, stop.clone()).await?;
        return promote(service, run, None, stop).await;
    }
    let mut mode = run.mode.clone();
    let mut cursor = cursor;
    // An expired start cursor invalidates the incomplete generation. Retry once
    // from a newly captured cursor; repeated expiry stays visible as a failure.
    for attempt in 0..2 {
        let start = if mode == "full" {
            let profile = service
                .accounts
                .gmail_get(&run.account_id, &["profile"], &[], ENVELOPE_LIMIT)
                .await?;
            opaque(&profile, "historyId")?
        } else {
            cursor.take().ok_or(MailError::Provider)?
        };
        service
            .store
            .begin_sync_run(run.account_id.clone(), run.id.clone(), Some(start.clone()))
            .await?;
        let labels = service
            .accounts
            .gmail_get(&run.account_id, &["labels"], &[], PAGE_LIMIT)
            .await?;
        let labels = optional_array(&labels, "labels")?;
        if labels.len() > 4096 {
            return Err(MailError::TooLarge);
        }
        for label in labels {
            service
                .store
                .stage_mail_collection(
                    run.account_id.clone(),
                    run.id.clone(),
                    opaque(label, "id")?,
                    label.get("name").and_then(Value::as_str).map(str::to_owned),
                    label
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned(),
                )
                .await?;
        }
        let mut processed = 0;
        if mode == "full" {
            processed = full(service, run, stop.clone()).await?;
        }
        match history(service, run, &start, &mut processed, stop.clone()).await {
            Ok(final_cursor) => return promote(service, run, Some(final_cursor), stop).await,
            Err(MailError::HistoryExpired) if attempt == 0 => {
                service
                    .store
                    .reset_mail_run(run.account_id.clone(), run.id.clone())
                    .await?;
                mode = "full".into();
            }
            other => return other.map(|_| ()),
        }
    }
    Err(MailError::HistoryExpired)
}
async fn full(
    service: &MailSync,
    run: &SyncRun,
    stop: watch::Receiver<bool>,
) -> Result<u64, MailError> {
    let mut page: Option<String> = None;
    let mut pages = BTreeSet::new();
    let mut processed = 0;
    loop {
        let mut query = vec![("maxResults", "100"), ("includeSpamTrash", "true")];
        if let Some(page) = &page {
            query.push(("pageToken", page));
        }
        let response = service
            .accounts
            .gmail_get(&run.account_id, &["messages"], &query, PAGE_LIMIT)
            .await?;
        let items = optional_array(&response, "messages")?;
        if items.len() > 1000 {
            return Err(MailError::Provider);
        }
        for item in items {
            ingest(service, run, &opaque(item, "id")?, stop.clone()).await?;
            processed += 1;
        }
        let next = next_page(&response, &mut pages)?;
        service
            .store
            .sync_run_progress(
                run.account_id.clone(),
                run.id.clone(),
                processed,
                next.clone(),
            )
            .await?;
        page = next;
        if page.is_none() {
            return Ok(processed);
        }
    }
}
async fn history(
    service: &MailSync,
    run: &SyncRun,
    start: &str,
    processed: &mut u64,
    stop: watch::Receiver<bool>,
) -> Result<String, MailError> {
    let mut page: Option<String> = None;
    let mut pages = BTreeSet::new();
    loop {
        let mut query = vec![("startHistoryId", start), ("maxResults", "100")];
        if let Some(page) = &page {
            query.push(("pageToken", page));
        }
        let response = match service
            .accounts
            .gmail_get(&run.account_id, &["history"], &query, PAGE_LIMIT)
            .await
        {
            Err(MailError::NotFound) => return Err(MailError::HistoryExpired),
            other => other?,
        };
        let mut ids = BTreeSet::new();
        for record in optional_array(&response, "history")? {
            for item in optional_array(record, "messages")? {
                ids.insert(opaque(item, "id")?);
            }
            for kind in [
                "messagesAdded",
                "messagesDeleted",
                "labelsAdded",
                "labelsRemoved",
            ] {
                for item in optional_array(record, kind)? {
                    ids.insert(opaque(&item["message"], "id")?);
                }
            }
        }
        for id in ids {
            ingest(service, run, &id, stop.clone()).await?;
            *processed += 1;
        }
        let next = next_page(&response, &mut pages)?;
        service
            .store
            .sync_run_progress(
                run.account_id.clone(),
                run.id.clone(),
                *processed,
                next.clone(),
            )
            .await?;
        page = next;
        if page.is_none() {
            return opaque(&response, "historyId");
        }
    }
}
async fn ingest(
    service: &MailSync,
    run: &SyncRun,
    id: &str,
    stop: watch::Receiver<bool>,
) -> Result<(), MailError> {
    let metadata = match service
        .accounts
        .gmail_get(
            &run.account_id,
            &["messages", id],
            &[("format", "metadata")],
            ENVELOPE_LIMIT,
        )
        .await
    {
        Err(MailError::NotFound) => {
            return service
                .store
                .stage_mail_deletion(run.account_id.clone(), run.id.clone(), id.into())
                .await
                .map_err(Into::into)
        }
        other => other?,
    };
    let limit = service.accounts.http.max_payload_bytes;
    let (metadata, encoded, availability) = match service
        .accounts
        .gmail_get(
            &run.account_id,
            &["messages", id],
            &[("format", "raw")],
            limit.div_ceil(3) * 4 + ENVELOPE_LIMIT,
        )
        .await
    {
        Ok(mut raw) => {
            let encoded = match raw.as_object_mut().and_then(|v| v.remove("raw")) {
                Some(Value::String(s)) => Some(s),
                None => None,
                _ => return Err(MailError::Provider),
            };
            let state = if encoded.is_some() {
                "available"
            } else {
                "missing"
            };
            // Raw-format responses carry current label/history metadata. Keep the
            // metadata-only headers when those are absent from the raw response.
            let mut combined = metadata;
            let object = combined.as_object_mut().ok_or(MailError::Provider)?;
            object.extend(raw.as_object().ok_or(MailError::Provider)?.clone());
            (combined, encoded, state)
        }
        Err(MailError::TooLarge) => (metadata, None, "too_large"),
        Err(MailError::NotFound) => {
            return service
                .store
                .stage_mail_deletion(run.account_id.clone(), run.id.clone(), id.into())
                .await
                .map_err(Into::into)
        }
        Err(error) => return Err(error),
    };
    let expected = id.to_owned();
    let mail = tokio::task::spawn_blocking(move || {
        decode(metadata, encoded, availability, limit, &expected)
    })
    .await
    .map_err(|_| MailError::Provider)??;
    service.checkpoint("gmail-before-page-commit", stop).await?;
    service
        .store
        .stage_mail(run.account_id.clone(), run.id.clone(), mail)
        .await?;
    Ok(())
}
fn decode(
    metadata: Value,
    encoded: Option<String>,
    availability: &str,
    limit: usize,
    expected: &str,
) -> Result<StagedMail, MailError> {
    let parsed: Message =
        serde_json::from_value(metadata.clone()).map_err(|_| MailError::Provider)?;
    if parsed.id != expected {
        return Err(MailError::Provider);
    }
    let internal_date_ms = parsed
        .internal_date
        .map(|s| {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(MailError::Provider);
            }
            s.parse::<i64>().map_err(|_| MailError::Provider)
        })
        .transpose()?;
    let mut availability = availability.to_owned();
    let raw = encoded
        .map(|s| {
            URL_SAFE_NO_PAD
                .decode(&s)
                .or_else(|_| URL_SAFE.decode(&s))
                .map_err(|_| MailError::Provider)
        })
        .transpose()?;
    let raw = match raw {
        Some(bytes) if bytes.len() > limit => {
            availability = "too_large".into();
            None
        }
        other => other,
    };
    let decoded = raw
        .as_ref()
        .and_then(|bytes| match decode_mime(bytes, limit) {
            Ok(value) => Some(value),
            Err(_) => {
                availability = "unparsed".into();
                None
            }
        });
    let headers = parsed.payload.map(|p| p.headers).unwrap_or_default();
    let subject = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("subject"))
        .map(|h| h.value.clone());
    Ok(StagedMail {
        provider_id: parsed.id,
        thread_id: parsed.thread_id,
        history_id: parsed.history_id,
        internal_date_ms,
        provider_json: serde_json::to_string(&metadata).map_err(|_| MailError::Provider)?,
        subject,
        headers,
        labels: parsed.label_ids,
        raw,
        decoded,
        availability,
    })
}
async fn promote(
    service: &MailSync,
    run: &SyncRun,
    cursor: Option<String>,
    stop: watch::Receiver<bool>,
) -> Result<(), MailError> {
    service
        .checkpoint("gmail-before-promotion", stop.clone())
        .await?;
    service
        .store
        .promote_mail(
            run.account_id.clone(),
            run.id.clone(),
            cursor,
            service.now()?,
        )
        .await?;
    service.checkpoint("gmail-after-promotion", stop).await?;
    Ok(())
}
fn opaque(value: &Value, key: &str) -> Result<String, MailError> {
    let value = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(MailError::Provider)?;
    if value.is_empty() || value.len() > 8192 || value.chars().any(char::is_control) {
        return Err(MailError::Provider);
    }
    Ok(value.into())
}
fn optional_array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], MailError> {
    match value.get(key) {
        None => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        _ => Err(MailError::Provider),
    }
}
fn next_page(value: &Value, pages: &mut BTreeSet<String>) -> Result<Option<String>, MailError> {
    if value.get("nextPageToken").is_none() {
        return Ok(None);
    }
    let token = opaque(value, "nextPageToken")?;
    if pages.len() >= 100_000 || !pages.insert(token.clone()) {
        return Err(MailError::Provider);
    }
    Ok(Some(token))
}
