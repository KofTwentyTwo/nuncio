use super::*;
use crate::store::StoredBlob;
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;

pub(crate) async fn find_client(
    connection: &mut Connection,
    intent: &SmtpIntent,
    floor: NonZeroU32,
    message_id: &str,
    expected: &StoredBlob,
    limit: usize,
) -> Result<Option<SentCopyResult>, TransferError> {
    let ids = super::server::search(connection, intent, floor, message_id).await?;
    let Some(uid) = ids.first() else {
        return Ok(None);
    };
    let placement = ImapPlacement::new(
        intent.account_id,
        intent.mailbox_id,
        intent.uid_validity.get(),
        *uid,
    )
    .map_err(|_| MailError::Protocol)?;
    let observed = super::observe(connection, intent, placement, limit).await?;
    let expected = expected.clone();
    let compared = tokio::task::spawn_blocking(move || {
        let raw = observed.mail.raw.as_ref().ok_or(MailError::Protocol)?;
        let byte_length = raw.len() as u64;
        let sha256 = hex::encode(Sha256::digest(raw));
        Ok::<_, MailError>(
            (byte_length == expected.byte_length && sha256 == expected.sha256).then_some(observed),
        )
    })
    .await
    .map_err(|_| MailError::Unavailable)??;
    if super::server::search(connection, intent, floor, message_id).await? != ids {
        return Err(MailError::Unavailable.into());
    }
    Ok(compared)
}
