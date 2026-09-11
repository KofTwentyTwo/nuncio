use super::{
    command::{complete, complete_with_raw},
    Connection, MailError,
};
mod names;
use crate::domain::imap::{ImapFlags, ImapMailboxState};
use async_imap::imap_proto::{AttributeValue, MailboxDatum, NameAttribute, Response, ResponseCode};
use std::collections::BTreeSet;

pub(super) async fn folders(
    connection: &mut Connection,
) -> Result<Vec<ImapMailboxState>, MailError> {
    let folders = complete_with_raw(
        &mut connection.session,
        "LIST \"\" \"*\"",
        4 * 1024 * 1024,
        8192,
        |response, raw| {
            let Response::MailboxData(MailboxDatum::List {
                name_attributes,
                delimiter,
                name: _,
            }) = response
            else {
                return Ok(None);
            };
            let delimiter = delimiter
                .as_deref()
                .map(|s| {
                    let mut chars = s.chars();
                    let c = chars.next().ok_or(MailError::Protocol)?;
                    if chars.next().is_some() {
                        return Err(MailError::Protocol);
                    }
                    Ok(c)
                })
                .transpose()?;
            let attributes = name_attributes
                .iter()
                .map(|a| {
                    Ok(format!(
                        "\\{}",
                        match a {
                            NameAttribute::NoInferiors => "Noinferiors",
                            NameAttribute::NoSelect => "Noselect",
                            NameAttribute::Marked => "Marked",
                            NameAttribute::Unmarked => "Unmarked",
                            NameAttribute::All => "All",
                            NameAttribute::Archive => "Archive",
                            NameAttribute::Drafts => "Drafts",
                            NameAttribute::Flagged => "Flagged",
                            NameAttribute::Junk => "Junk",
                            NameAttribute::Sent => "Sent",
                            NameAttribute::Trash => "Trash",
                            NameAttribute::Extension(s) =>
                                s.strip_prefix('\\').ok_or(MailError::Protocol)?,
                            _ => return Err(MailError::Protocol),
                        }
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(ImapMailboxState {
                name: names::mailbox(raw)?,
                delimiter,
                attributes,
                uid_validity: None,
                uid_next: None,
                highest_mod_seq: None,
            }))
        },
    )
    .await?;
    let mut unique = BTreeSet::new();
    for folder in &folders {
        if !unique.insert(folder.name.as_str()) {
            return Err(MailError::Protocol);
        }
    }
    if !unique.contains("INBOX") {
        return Err(MailError::Protocol);
    }
    Ok(folders)
}
pub(super) async fn examine(
    connection: &mut Connection,
    folder: ImapMailboxState,
) -> Result<(ImapMailboxState, u32), MailError> {
    select(connection, folder, false).await
}
pub(super) async fn select(
    connection: &mut Connection,
    mut folder: ImapMailboxState,
    writable: bool,
) -> Result<(ImapMailboxState, u32), MailError> {
    let mut read_write = false;
    let mut count = None;
    let name = format!(
        "\"{}\"",
        folder
            .name
            .wire_name()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    );
    folder.uid_validity = None;
    folder.uid_next = None;
    folder.highest_mod_seq = None;
    complete::<()>(
        &mut connection.session,
        &format!("{} {name}", if writable { "SELECT" } else { "EXAMINE" }),
        256 * 1024,
        0,
        |response| {
            if let Response::MailboxData(MailboxDatum::Exists(value)) = response {
                count = Some(*value);
            }
            let code = match response {
                Response::Data { code, .. } | Response::Done { code, .. } => code.as_ref(),
                _ => None,
            };
            match code {
                Some(ResponseCode::ReadWrite) => read_write = true,
                Some(ResponseCode::ReadOnly) if writable => return Err(MailError::Unsupported),
                Some(ResponseCode::UidValidity(v)) => folder.uid_validity = Some(*v),
                Some(ResponseCode::UidNext(v)) => folder.uid_next = Some(*v),
                Some(ResponseCode::HighestModSeq(v)) => folder.highest_mod_seq = Some(*v),
                Some(ResponseCode::UidNotSticky) => return Err(MailError::Unsupported),
                _ => {}
            }
            Ok(None)
        },
    )
    .await?;
    if writable && !read_write {
        return Err(MailError::Protocol);
    }
    Ok((
        folder.validated().map_err(|_| MailError::Protocol)?,
        count.ok_or(MailError::Protocol)?,
    ))
}
pub(super) async fn inventory(
    connection: &mut Connection,
    next: u32,
    count: u32,
) -> Result<Vec<u32>, MailError> {
    if next == 0 || count > 1_000_000 || (count > 0 && next == 1) {
        return Err(MailError::Protocol);
    }
    // RFC3501 6.4.8: intersect bounded sequence positions with captured UIDs.
    // This bounds each response even when one old message has a very high UID.
    let mut result = BTreeSet::new();
    let mut low = 1u32;
    while low <= count {
        let high = low.saturating_add(4095).min(count);
        let found = complete(
            &mut connection.session,
            &format!("UID SEARCH {low}:{high} UID 1:{}", next - 1),
            128 * 1024,
            0,
            |response| match response {
                Response::MailboxData(MailboxDatum::Search(uids)) => Ok(Some(uids.clone())),
                Response::Expunge(_) | Response::Vanished { .. } => Err(MailError::Unavailable),
                Response::MailboxData(MailboxDatum::Exists(value)) if *value != count => {
                    Err(MailError::Unavailable)
                }
                _ => Ok(None),
            },
        )
        .await?;
        if found.len() != 1 || found[0].len() > (high - low + 1) as usize {
            return Err(MailError::Protocol);
        }
        for uid in found.into_iter().flatten() {
            if uid == 0 || uid >= next || !result.insert(uid) {
                return Err(MailError::Protocol);
            }
        }
        low = high + 1;
    }
    if result.len() != count as usize {
        return Err(MailError::Unavailable);
    }
    Ok(result.into_iter().collect())
}
pub(super) struct Metadata {
    pub uid: u32,
    pub flags: ImapFlags,
    pub size: u32,
    pub date: i64,
}
pub(super) async fn metadata(
    connection: &mut Connection,
    batch: &str,
) -> Result<Vec<Metadata>, MailError> {
    complete(
        &mut connection.session,
        &format!("UID FETCH {batch} (UID FLAGS RFC822.SIZE INTERNALDATE)"),
        2 * 1024 * 1024,
        0,
        |response| {
            let Response::Fetch(_, values) = response else {
                return Ok(None);
            };
            let (mut uid, mut flags, mut size, mut date) = (None, None, None, None);
            for value in values {
                match value {
                    AttributeValue::Uid(v) if uid.is_none() => uid = Some(*v),
                    AttributeValue::Flags(v) if flags.is_none() => {
                        flags = Some(
                            ImapFlags::new(v.iter().map(|s| s.to_string()).collect())
                                .map_err(|_| MailError::Protocol)?,
                        )
                    }
                    AttributeValue::Rfc822Size(v) if size.is_none() => size = Some(*v),
                    AttributeValue::InternalDate(v) if date.is_none() => {
                        date = Some(
                            chrono::DateTime::parse_from_str(v.trim(), "%d-%b-%Y %H:%M:%S %z")
                                .map_err(|_| MailError::Protocol)?
                                .timestamp_millis(),
                        )
                    }
                    _ => return Err(MailError::Protocol),
                }
            }
            Ok(Some(Metadata {
                uid: uid.filter(|v| *v != 0).ok_or(MailError::Protocol)?,
                flags: flags.ok_or(MailError::Protocol)?,
                size: size.ok_or(MailError::Protocol)?,
                date: date.ok_or(MailError::Protocol)?,
            }))
        },
    )
    .await
}
pub(super) async fn body(
    connection: &mut Connection,
    uid: u32,
    size: u32,
    limit: usize,
) -> Result<Vec<u8>, MailError> {
    let items = complete(
        &mut connection.session,
        &format!("UID FETCH {uid} (UID BODY.PEEK[])"),
        limit + 256 * 1024,
        limit,
        |response| {
            let Response::Fetch(_, values) = response else {
                return Ok(None);
            };
            let (mut seen, mut body) = (None, None);
            for value in values {
                match value {
                    AttributeValue::Uid(value) if seen.is_none() => seen = Some(*value),
                    AttributeValue::BodySection {
                        section: None,
                        index: None,
                        data: Some(bytes),
                    } if body.is_none() => body = Some(bytes.to_vec()),
                    _ => return Err(MailError::Protocol),
                }
            }
            if seen != Some(uid) {
                return Err(MailError::Protocol);
            }
            let body = body.ok_or(MailError::Protocol)?;
            if body.len() != size as usize || body.len() > limit {
                return Err(MailError::Protocol);
            }
            Ok(Some(body))
        },
    )
    .await?;
    if items.len() != 1 {
        return Err(MailError::Protocol);
    }
    items.into_iter().next().ok_or(MailError::Protocol)
}

#[cfg(test)]
mod tests;
