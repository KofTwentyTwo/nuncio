use super::{read, MailError as WireError};
use crate::{
    domain::{
        identity::AccountId,
        imap::{uid_batches, ImapPlacement},
        mail::decode_mime,
    },
    mail::{MailError, MailSync},
    store::{StagedMail, SyncRun},
};
use std::{collections::BTreeSet, num::NonZeroU32};
use tokio::sync::watch;
impl From<WireError> for MailError {
    fn from(error: WireError) -> Self {
        match error {
            WireError::Unavailable => Self::Unavailable,
            WireError::Protocol => Self::Provider,
            other => Self::Account(other.into()),
        }
    }
}
pub(crate) async fn sync(
    service: &MailSync,
    run: &SyncRun,
    stop: watch::Receiver<bool>,
) -> Result<(), MailError> {
    let account: AccountId = run.account_id.parse().map_err(|_| MailError::Provider)?;
    service
        .store
        .begin_sync_run(run.account_id.clone(), run.id.clone(), None)
        .await?;
    let mut connection = service.accounts.imap_session(&run.account_id).await?;
    let folders = read::folders(&mut connection).await?;
    let mut processed = 0;
    for folder in folders {
        if folder
            .attributes
            .iter()
            .any(|a| a.eq_ignore_ascii_case("\\Noselect"))
        {
            service
                .store
                .stage_imap_mailbox(run.account_id.clone(), run.id.clone(), folder)
                .await?;
            continue;
        }
        let (folder, count) = read::examine(&mut connection, folder).await?;
        let epoch = folder.uid_validity.ok_or(MailError::Provider)?;
        let next = folder.uid_next.ok_or(MailError::Provider)?;
        let id = service
            .store
            .stage_imap_mailbox(run.account_id.clone(), run.id.clone(), folder.clone())
            .await?;
        let uids = read::inventory(&mut connection, next, count).await?;
        for group in uids.chunks(256) {
            let numbers = group
                .iter()
                .map(|u| NonZeroU32::new(*u).ok_or(MailError::Provider))
                .collect::<Result<Vec<_>, _>>()?;
            let batch = uid_batches(&numbers, 256)
                .map_err(|_| MailError::Provider)?
                .into_iter()
                .next()
                .ok_or(MailError::Provider)?;
            let messages = read::metadata(&mut connection, &batch).await?;
            let mut seen = BTreeSet::new();
            for message in messages {
                if group.binary_search(&message.uid).is_err() || !seen.insert(message.uid) {
                    return Err(MailError::Provider);
                }
                let placement = ImapPlacement::new(account, id, epoch, message.uid)
                    .map_err(|_| MailError::Provider)?;
                ingest(
                    service,
                    run,
                    &mut connection,
                    placement,
                    message,
                    stop.clone(),
                )
                .await?;
                processed += 1;
            }
            if seen.len() != group.len() {
                return Err(MailError::Provider);
            }
            service
                .store
                .sync_run_progress(run.account_id.clone(), run.id.clone(), processed, None)
                .await?;
        }
        // A reset, append or expunge during collection invalidates this generation.
        // Nothing staged becomes visible until every mailbox finished positively.
        let (final_folder, final_count) = read::examine(&mut connection, folder.clone()).await?;
        if final_count != count
            || final_folder.uid_validity != folder.uid_validity
            || final_folder.uid_next != folder.uid_next
            || final_folder.highest_mod_seq != folder.highest_mod_seq
            || read::inventory(&mut connection, next, count).await? != uids
        {
            return Err(MailError::Unavailable);
        }
        service
            .store
            .stage_absent_imap_mail(run.account_id.clone(), run.id.clone(), id, epoch, uids)
            .await?;
    }
    service
        .checkpoint("imap-before-promotion", stop.clone())
        .await?;
    service
        .store
        .promote_mail(run.account_id.clone(), run.id.clone(), None, service.now()?)
        .await?;
    service.checkpoint("imap-after-promotion", stop).await?;
    Ok(())
}

async fn ingest(
    service: &MailSync,
    run: &SyncRun,
    connection: &mut super::Connection,
    placement: ImapPlacement,
    message: read::Metadata,
    stop: watch::Receiver<bool>,
) -> Result<(), MailError> {
    let cached = run.mode == "delta"
        && service
            .store
            .stage_cached_imap_mail(run.id.clone(), placement, message.flags.clone())
            .await?;
    if !cached {
        let limit = service.accounts.http.max_payload_bytes;
        let raw = if message.size as usize <= limit {
            Some(read::body(connection, message.uid, message.size, limit).await?)
        } else {
            None
        };
        let provider_id = placement.provider_id().map_err(|_| MailError::Provider)?;
        let staged = tokio::task::spawn_blocking(move || {
            let decoded = raw
                .as_ref()
                .and_then(|bytes| decode_mime(bytes, limit).ok());
            let availability = if raw.is_none() {
                "too_large"
            } else if decoded.is_none() {
                "unparsed"
            } else {
                "available"
            };
            StagedMail {
                provider_id,
                thread_id: None,
                history_id: None,
                internal_date_ms: Some(message.date),
                provider_json: "{}".into(),
                subject: None,
                headers: vec![],
                labels: vec![],
                raw,
                decoded,
                availability: availability.into(),
            }
        })
        .await
        .map_err(|_| MailError::Provider)?;
        service
            .checkpoint("imap-before-message-stage", stop.clone())
            .await?;
        service
            .store
            .stage_imap_mail(run.id.clone(), placement, message.flags, staged)
            .await?;
    }
    Ok(())
}

pub(crate) async fn fetch(
    service: &MailSync,
    run: &SyncRun,
    message: String,
    stop: watch::Receiver<bool>,
) -> Result<(), MailError> {
    service
        .store
        .begin_sync_run(run.account_id.clone(), run.id.clone(), None)
        .await?;
    let account: AccountId = run.account_id.parse().map_err(|_| MailError::Provider)?;
    let cached = service
        .store
        .get_mail(run.account_id.clone(), message)
        .await?;
    let placement = ImapPlacement::from_provider_id(account, &cached.message.provider_id)
        .map_err(|_| MailError::Provider)?;
    let folder = service
        .store
        .imap_mailboxes(run.account_id.clone())
        .await?
        .into_iter()
        .find(|m| m.id == placement.mailbox_id && !m.retired)
        .ok_or(MailError::NotFound)?;
    let mut connection = service.accounts.imap_session(&run.account_id).await?;
    let (folder, _) = read::examine(&mut connection, folder.state).await?;
    if folder.uid_validity != Some(placement.uid_validity.get())
        || folder
            .uid_next
            .is_none_or(|next| placement.uid.get() >= next)
    {
        return Err(MailError::NotFound);
    }
    let id = service
        .store
        .stage_imap_mailbox(run.account_id.clone(), run.id.clone(), folder.clone())
        .await?;
    if id != placement.mailbox_id {
        return Err(MailError::NotFound);
    }
    let messages = read::metadata(&mut connection, &placement.uid.to_string()).await?;
    if messages.is_empty() {
        return Err(MailError::NotFound);
    }
    if messages.len() != 1 {
        return Err(MailError::Provider);
    }
    let message = messages.into_iter().next().ok_or(MailError::NotFound)?;
    if message.uid != placement.uid.get() {
        return Err(MailError::Provider);
    }
    ingest(
        service,
        run,
        &mut connection,
        placement,
        message,
        stop.clone(),
    )
    .await?;
    let (after, _) = read::examine(&mut connection, folder.clone()).await?;
    if after.uid_validity != folder.uid_validity {
        return Err(MailError::NotFound);
    }
    service
        .store
        .sync_run_progress(run.account_id.clone(), run.id.clone(), 1, None)
        .await?;
    service
        .checkpoint("imap-before-promotion", stop.clone())
        .await?;
    service
        .store
        .promote_mail(run.account_id.clone(), run.id.clone(), None, service.now()?)
        .await?;
    service.checkpoint("imap-after-promotion", stop).await?;
    Ok(())
}
