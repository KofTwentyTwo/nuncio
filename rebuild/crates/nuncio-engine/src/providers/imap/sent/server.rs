use super::*;
use crate::domain::submission::SentFingerprint;
use async_imap::imap_proto::MailboxDatum;
use std::{collections::BTreeSet, num::NonZeroU32};

pub(crate) async fn find_server(
    connection: &mut Connection,
    intent: &SmtpIntent,
    floor: NonZeroU32,
    message_id: &str,
    limit: usize,
) -> Result<Option<(SentCopyResult, SentFingerprint)>, TransferError> {
    let expected = intent
        .sent_fingerprint
        .clone()
        .ok_or(TransferError::Conflict("smtp_sent_fingerprint_unavailable"))?;
    let ids = search(connection, intent, floor, message_id).await?;
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
    let compared = tokio::task::spawn_blocking(move || {
        let fingerprint =
            SentFingerprint::from_mime(observed.mail.raw.as_deref().ok_or(MailError::Protocol)?)
                .map_err(|_| MailError::Protocol)?;
        Ok::<_, MailError>(
            expected
                .matches(&fingerprint)
                .then_some((observed, fingerprint)),
        )
    })
    .await
    .map_err(|_| MailError::Unavailable)??;
    // Recheck the complete candidate set after the body read. Another client
    // may add/remove a matching placement while the literal is being fetched.
    if search(connection, intent, floor, message_id).await? != ids {
        return Err(MailError::Unavailable.into());
    }
    Ok(compared)
}
pub(super) async fn search(
    connection: &mut Connection,
    intent: &SmtpIntent,
    floor: NonZeroU32,
    message_id: &str,
) -> Result<Vec<u32>, TransferError> {
    if message_id.is_empty()
        || message_id.len() > 998
        || !message_id.is_ascii()
        || message_id.chars().any(char::is_control)
    {
        return Err(MailError::Invalid.into());
    }
    let id = message_id.replace('\\', "\\\\").replace('"', "\\\"");
    let (state, count) = super::selected(connection, intent).await?;
    let next = state.uid_next.ok_or(MailError::Protocol)?;
    if count > 1_000_000 {
        return Err(MailError::Protocol.into());
    }
    if next <= floor.get() {
        return Ok(vec![]);
    }
    let mut ids = BTreeSet::new();
    let mut low = 1u32;
    while low <= count {
        let high = low.saturating_add(4095).min(count);
        let responses = super::super::command::complete(
            &mut connection.session,
            &format!(
                "UID SEARCH {low}:{high} UID {floor}:{} HEADER Message-ID \"<{id}>\"",
                next - 1
            ),
            128 * 1024,
            0,
            |response| match response {
                Response::MailboxData(MailboxDatum::Search(uids)) => Ok(Some(uids.clone())),
                Response::Expunge(_) | Response::Vanished { .. } => Err(MailError::Unavailable),
                Response::MailboxData(MailboxDatum::Exists(current)) if *current != count => {
                    Err(MailError::Unavailable)
                }
                _ => Ok(None),
            },
        )
        .await?;
        if responses.len() != 1 || responses[0].len() > (high - low + 1) as usize {
            return Err(MailError::Protocol.into());
        }
        for uid in responses.into_iter().flatten() {
            if uid < floor.get() || uid >= next || !ids.insert(uid) {
                return Err(MailError::Protocol.into());
            }
        }
        if ids.len() > 1 {
            return Err(TransferError::Conflict(
                if intent.sent_policy == crate::domain::imap_account::SentPolicy::Server {
                    "smtp_server_sent_ambiguous"
                } else {
                    "imap_client_sent_ambiguous"
                },
            ));
        }
        low = high + 1;
    }
    Ok(ids.into_iter().collect())
}

#[cfg(test)]
mod tests;
