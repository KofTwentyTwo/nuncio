use super::{command::complete, read, Connection, MailError};
use crate::{domain::imap::ImapMessageState, store::ImapFlagPayload};

pub(crate) enum Observation {
    Message(ImapMessageState),
    Missing,
    EpochChanged,
}
pub(crate) async fn observe(
    connection: &mut Connection,
    payload: &ImapFlagPayload,
    writable: bool,
) -> Result<Observation, MailError> {
    let Some(folder) = read::folders(connection)
        .await?
        .into_iter()
        .find(|folder| folder.name == payload.mailbox)
    else {
        return Ok(Observation::Missing);
    };
    if folder
        .attributes
        .iter()
        .any(|a| a.eq_ignore_ascii_case("\\Noselect"))
    {
        return Ok(Observation::Missing);
    }
    let (folder, _) = read::select(connection, folder, writable).await?;
    if folder.uid_validity != Some(payload.placement.uid_validity.get()) {
        return Ok(Observation::EpochChanged);
    }
    selected(connection, payload).await
}
async fn selected(
    connection: &mut Connection,
    payload: &ImapFlagPayload,
) -> Result<Observation, MailError> {
    let mut messages = read::metadata(connection, &payload.placement.uid.to_string()).await?;
    if messages.is_empty() {
        return Ok(Observation::Missing);
    }
    if messages.len() != 1 {
        return Err(MailError::Protocol);
    }
    let message = messages.pop().ok_or(MailError::Protocol)?;
    if message.uid != payload.placement.uid.get() {
        return Err(MailError::Protocol);
    }
    Ok(Observation::Message(ImapMessageState {
        placement: payload.placement,
        flags: message.flags,
    }))
}
pub(crate) async fn change(
    connection: &mut Connection,
    payload: &ImapFlagPayload,
) -> Result<Observation, MailError> {
    // Change just the requested flag. Replacing FLAGS would erase concurrent
    // keywords, replies, or other state added by another mail client.
    complete::<()>(
        &mut connection.session,
        &format!(
            "UID STORE {} {}FLAGS.SILENT ({})",
            payload.placement.uid,
            if payload.present { "+" } else { "-" },
            payload.flag.wire_name()
        ),
        256 * 1024,
        0,
        |_| Ok(None),
    )
    .await?;
    selected(connection, payload).await
}
