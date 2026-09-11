use super::{command::complete, read, Connection, MailError};
use crate::{
    domain::{
        imap::{ImapFlags, ImapMailboxState, ImapPlacement},
        mail::decode_mime,
    },
    store::{ImapCopyProof, ImapTransferMode, ImapTransferPayload, ImapTransferResult, StagedMail},
};
use async_imap::imap_proto::{Response, ResponseCode, Status, UidSetMember};

#[derive(Debug, thiserror::Error)]
pub(crate) enum TransferError {
    #[error(transparent)]
    Wire(#[from] MailError),
    #[error("Transfer resource changed: {0}")]
    Conflict(&'static str),
}
pub(crate) struct CopyOutcome {
    pub proof: Option<ImapCopyProof>,
    pub acknowledged: bool,
}
pub(super) async fn folder(
    connection: &mut Connection,
    name: &crate::domain::imap::MailboxName,
) -> Result<ImapMailboxState, TransferError> {
    read::folders(connection)
        .await?
        .into_iter()
        .find(|f| {
            &f.name == name
                && !f
                    .attributes
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case("\\Noselect"))
        })
        .ok_or(TransferError::Conflict("imap_folder_missing"))
}
pub(crate) async fn destination(
    connection: &mut Connection,
    payload: &ImapTransferPayload,
) -> Result<ImapMailboxState, TransferError> {
    let target = folder(connection, &payload.destination_mailbox).await?;
    let (state, _) = read::examine(connection, target).await?;
    if state.uid_validity != Some(payload.destination_uid_validity.get()) {
        return Err(TransferError::Conflict("imap_destination_epoch_changed"));
    }
    Ok(state)
}
pub(crate) async fn source(
    connection: &mut Connection,
    payload: &ImapTransferPayload,
    writable: bool,
) -> Result<Option<ImapFlags>, TransferError> {
    let source = folder(connection, &payload.source_mailbox).await?;
    let (state, _) = read::select(connection, source, writable).await?;
    if state.uid_validity != Some(payload.source.uid_validity.get()) {
        return Err(TransferError::Conflict("imap_source_epoch_changed"));
    }
    Ok(metadata(connection, payload.source.uid.get())
        .await?
        .map(|m| m.flags))
}
pub(super) async fn metadata(
    connection: &mut Connection,
    uid: u32,
) -> Result<Option<read::Metadata>, MailError> {
    let mut items = read::metadata(connection, &uid.to_string()).await?;
    if items.len() > 1 || items.first().is_some_and(|m| m.uid != uid) {
        return Err(MailError::Protocol);
    }
    Ok(items.pop())
}
pub(super) fn single(set: &[UidSetMember]) -> Result<u32, MailError> {
    let uid = match set {
        [UidSetMember::Uid(uid)] => *uid,
        [UidSetMember::UidRange(range)] if range.start() == range.end() => *range.start(),
        _ => return Err(MailError::Protocol),
    };
    if uid == 0 {
        return Err(MailError::Protocol);
    }
    Ok(uid)
}
pub(crate) async fn copy_or_move(
    connection: &mut Connection,
    payload: &ImapTransferPayload,
    mode: ImapTransferMode,
) -> CopyOutcome {
    let name = payload
        .destination_mailbox
        .wire_name()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let verb = if mode == ImapTransferMode::Move {
        "MOVE"
    } else {
        "COPY"
    };
    let mut proof = None;
    let result = complete::<()>(
        &mut connection.session,
        &format!("UID {verb} {} \"{name}\"", payload.source.uid),
        256 * 1024,
        0,
        |response| {
            let code = match response {
                Response::Data {
                    status: Status::Ok,
                    code,
                    ..
                }
                | Response::Done {
                    status: Status::Ok,
                    code,
                    ..
                } => code.as_ref(),
                _ => None,
            };
            if let Some(ResponseCode::CopyUid(epoch, source, target)) = code {
                if single(source)? != payload.source.uid.get() {
                    return Err(MailError::Protocol);
                }
                let destination = ImapPlacement::new(
                    payload.source.account_id,
                    payload.destination,
                    *epoch,
                    single(target)?,
                )
                .map_err(|_| MailError::Protocol)?;
                let observed = ImapCopyProof {
                    source: payload.source,
                    destination,
                };
                if !payload.accepts(&observed) || proof.as_ref().is_some_and(|p| p != &observed) {
                    return Err(MailError::Protocol);
                }
                proof = Some(observed);
            }
            Ok(None)
        },
    )
    .await;
    // MOVE may report COPYUID before losing its final acknowledgement. Preserve
    // valid positive evidence on disconnect/NO, but not malformed protocol data.
    if matches!(
        result,
        Err(MailError::Protocol | MailError::Unsupported | MailError::Invalid)
    ) {
        proof = None;
    }
    CopyOutcome {
        proof,
        acknowledged: result.is_ok(),
    }
}
pub(crate) async fn mark_deleted(connection: &mut Connection, uid: u32) -> Result<(), MailError> {
    complete::<()>(
        &mut connection.session,
        &format!("UID STORE {uid} +FLAGS.SILENT (\\Deleted)"),
        256 * 1024,
        0,
        |_| Ok(None),
    )
    .await?;
    Ok(())
}
pub(crate) async fn expunge(connection: &mut Connection, uid: u32) -> Result<(), MailError> {
    if !connection.capabilities.uidplus {
        return Err(MailError::Unsupported);
    }
    complete::<()>(
        &mut connection.session,
        &format!("UID EXPUNGE {uid}"),
        256 * 1024,
        0,
        |_| Ok(None),
    )
    .await?;
    Ok(())
}
pub(crate) async fn fetch_destination(
    connection: &mut Connection,
    payload: &ImapTransferPayload,
    proof: ImapCopyProof,
    limit: usize,
) -> Result<ImapTransferResult, TransferError> {
    let mailbox = destination(connection, payload).await?;
    let item = metadata(connection, proof.destination.uid.get())
        .await?
        .ok_or(TransferError::Conflict("imap_copied_message_missing"))?;
    let raw = if item.size as usize <= limit {
        Some(read::body(connection, item.uid, item.size, limit).await?)
    } else {
        None
    };
    tokio::task::spawn_blocking(move || {
        let decoded = raw.as_ref().and_then(|raw| decode_mime(raw, limit).ok());
        let availability = if raw.is_none() {
            "too_large"
        } else if decoded.is_none() {
            "unparsed"
        } else {
            "available"
        };
        let mail = StagedMail {
            provider_id: proof
                .destination
                .provider_id()
                .map_err(|_| MailError::Protocol)?,
            thread_id: None,
            history_id: None,
            internal_date_ms: Some(item.date),
            provider_json: "{}".into(),
            subject: None,
            headers: vec![],
            labels: vec![],
            decoded,
            raw,
            availability: availability.into(),
        };
        Ok(ImapTransferResult {
            proof,
            mailbox,
            flags: item.flags,
            mail,
        })
    })
    .await
    .map_err(|_| MailError::Unavailable)?
}

#[cfg(test)]
mod tests;
