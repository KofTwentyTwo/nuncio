use super::{
    read,
    transfers::{self, TransferError},
    Connection, MailError,
};
use crate::{
    domain::{
        imap::{ImapMailboxState, ImapPlacement},
        mail::decode_mime,
    },
    store::{SentCopyResult, SmtpIntent, StagedMail},
};
use async_imap::imap_proto::{Response, ResponseCode, Status};
use tokio::io::AsyncWriteExt;
mod client;
mod server;
pub(crate) use client::find_client;
pub(crate) use server::find_server;

pub(crate) struct AppendOutcome {
    pub placement: Option<ImapPlacement>,
    pub acknowledged: bool,
    pub rejected: bool,
}
pub(crate) async fn mailbox(
    connection: &mut Connection,
    intent: &SmtpIntent,
) -> Result<ImapMailboxState, TransferError> {
    Ok(selected(connection, intent).await?.0)
}
async fn selected(
    connection: &mut Connection,
    intent: &SmtpIntent,
) -> Result<(ImapMailboxState, u32), TransferError> {
    let folder = transfers::folder(connection, &intent.mailbox).await?;
    let (state, count) = read::examine(connection, folder).await?;
    if state.uid_validity != Some(intent.uid_validity.get()) {
        return Err(TransferError::Conflict("imap_sent_epoch_changed"));
    }
    Ok((state, count))
}
pub(crate) async fn append(
    connection: &mut Connection,
    intent: &SmtpIntent,
    raw: &[u8],
) -> AppendOutcome {
    let mut placement = None;
    let mut rejected = false;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        append_inner(connection, intent, raw, &mut placement, &mut rejected),
    )
    .await
    .unwrap_or(Err(MailError::Unavailable));
    if matches!(
        result,
        Err(MailError::Invalid | MailError::Protocol | MailError::Unsupported)
    ) {
        placement = None;
        rejected = false;
    }
    AppendOutcome {
        placement,
        acknowledged: result.is_ok(),
        rejected,
    }
}
async fn append_inner(
    connection: &mut Connection,
    intent: &SmtpIntent,
    raw: &[u8],
    placement: &mut Option<ImapPlacement>,
    rejected: &mut bool,
) -> Result<(), MailError> {
    if raw.is_empty() || raw.len() > crate::domain::mail::MAX_PAYLOAD_BYTES {
        return Err(MailError::Invalid);
    }
    let name = intent
        .mailbox
        .wire_name()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let session = &mut connection.session;
    let resources = session.get_mut().resources();
    let _request = resources
        .request()
        .await
        .map_err(|_| MailError::Unavailable)?;
    session
        .get_mut()
        .set_limits(256 * 1024, 0)
        .map_err(|_| MailError::Protocol)?;
    let tag = session
        .run_command(format!("APPEND \"{name}\" (\\Seen) {{{}}}", raw.len()))
        .await
        .map_err(|_| MailError::Unavailable)?;
    let mut transmitted = false;
    for _ in 0..4096 {
        let response = session
            .read_response()
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    MailError::Unavailable
                } else {
                    MailError::Protocol
                }
            })?
            .ok_or(MailError::Unavailable)?;
        let (done, code) = match response.parsed() {
            Response::Continue { .. } => {
                if transmitted {
                    return Err(MailError::Protocol);
                }
                session
                    .as_mut()
                    .write_all(raw)
                    .await
                    .map_err(|_| MailError::Unavailable)?;
                session
                    .as_mut()
                    .write_all(b"\r\n")
                    .await
                    .map_err(|_| MailError::Unavailable)?;
                session
                    .as_mut()
                    .flush()
                    .await
                    .map_err(|_| MailError::Unavailable)?;
                transmitted = true;
                continue;
            }
            Response::Done {
                tag: observed,
                status,
                code,
                ..
            } => {
                if observed != &tag {
                    return Err(MailError::Protocol);
                }
                match status {
                    Status::Ok if transmitted => (true, code.as_ref()),
                    Status::No => {
                        // A complete negative APPEND response guarantees no partial
                        // copy. Conflicting positive mapping is not that proof.
                        *rejected = placement.is_none();
                        return Err(MailError::Unavailable);
                    }
                    Status::Bad => return Err(MailError::Unsupported),
                    _ => return Err(MailError::Protocol),
                }
            }
            Response::Data {
                status: Status::Bye,
                ..
            } => return Err(MailError::Unavailable),
            Response::Data {
                status: Status::Ok,
                code,
                ..
            } => (false, code.as_ref()),
            _ => (false, None),
        };
        if let Some(ResponseCode::AppendUid(epoch, uids)) = code {
            if !transmitted {
                return Err(MailError::Protocol);
            }
            let observed = ImapPlacement::new(
                intent.account_id,
                intent.mailbox_id,
                *epoch,
                transfers::single(uids)?,
            )
            .map_err(|_| MailError::Protocol)?;
            if !intent.accepts(observed) || placement.is_some_and(|p| p != observed) {
                return Err(MailError::Protocol);
            }
            *placement = Some(observed);
        }
        if done {
            return Ok(());
        }
    }
    Err(MailError::Protocol)
}
pub(crate) async fn observe(
    connection: &mut Connection,
    intent: &SmtpIntent,
    placement: ImapPlacement,
    limit: usize,
) -> Result<SentCopyResult, TransferError> {
    if !intent.accepts(placement) {
        return Err(MailError::Invalid.into());
    }
    let mailbox = mailbox(connection, intent).await?;
    let item = transfers::metadata(connection, placement.uid.get())
        .await?
        .ok_or(TransferError::Conflict("imap_sent_message_missing"))?;
    if item.size as usize > limit {
        return Err(MailError::Protocol.into());
    }
    let raw = read::body(connection, item.uid, item.size, limit).await?;
    tokio::task::spawn_blocking(move || {
        let decoded = decode_mime(&raw, limit).map_err(|_| MailError::Protocol)?;
        Ok(SentCopyResult {
            placement,
            mailbox,
            flags: item.flags,
            mail: StagedMail {
                provider_id: placement.provider_id().map_err(|_| MailError::Protocol)?,
                thread_id: None,
                history_id: None,
                internal_date_ms: Some(item.date),
                provider_json: "{}".into(),
                subject: None,
                headers: vec![],
                labels: vec![],
                decoded: Some(decoded),
                raw: Some(raw),
                availability: "available".into(),
            },
        })
    })
    .await
    .map_err(|_| MailError::Unavailable)?
}

#[cfg(test)]
mod tests;
