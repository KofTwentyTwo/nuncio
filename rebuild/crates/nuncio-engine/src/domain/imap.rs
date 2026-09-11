use super::{
    identity::{AccountId, ImapMailboxId},
    imap_account::validate_mailbox,
};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, num::NonZeroU32};

#[derive(Debug, thiserror::Error)]
#[error("Invalid IMAP mailbox, placement, flag or UID input")]
pub struct ImapDataError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapPlacement {
    pub account_id: AccountId,
    pub mailbox_id: ImapMailboxId,
    pub uid_validity: NonZeroU32,
    pub uid: NonZeroU32,
}
impl ImapPlacement {
    pub fn new(
        account_id: AccountId,
        mailbox_id: ImapMailboxId,
        uid_validity: u32,
        uid: u32,
    ) -> Result<Self, ImapDataError> {
        Ok(Self {
            account_id,
            mailbox_id,
            uid_validity: NonZeroU32::new(uid_validity).ok_or(ImapDataError)?,
            uid: NonZeroU32::new(uid).ok_or(ImapDataError)?,
        })
    }
    pub fn provider_id(&self) -> Result<String, ImapDataError> {
        // No RFC Message-ID, mutable folder name, or ambiguous delimiter join.
        serde_json::to_string(&(1, "imap", self)).map_err(|_| ImapDataError)
    }
    pub fn from_provider_id(account: AccountId, id: &str) -> Result<Self, ImapDataError> {
        if id.len() > 1024 {
            return Err(ImapDataError);
        }
        let (version, provider, placement): (u8, String, Self) =
            serde_json::from_str(id).map_err(|_| ImapDataError)?;
        if version != 1 || provider != "imap" || placement.account_id != account {
            return Err(ImapDataError);
        }
        Ok(placement)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MailboxName(String);
impl TryFrom<String> for MailboxName {
    type Error = ImapDataError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_unicode(&value)
    }
}
impl From<MailboxName> for String {
    fn from(value: MailboxName) -> Self {
        value.0
    }
}
impl MailboxName {
    pub fn from_unicode(name: &str) -> Result<Self, ImapDataError> {
        validate_mailbox(name).map_err(|_| ImapDataError)?;
        Ok(Self(if name.eq_ignore_ascii_case("INBOX") {
            "INBOX".into()
        } else {
            name.into()
        }))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn wire_name(&self) -> String {
        encode_name(&self.0)
    }
    pub fn from_wire(wire: &str) -> Result<Self, ImapDataError> {
        if wire.is_empty() || wire.len() > 8192 || !wire.bytes().all(|b| (0x20..=0x7e).contains(&b))
        {
            return Err(ImapDataError);
        }
        let mut out = String::new();
        let mut remaining = wire;
        while let Some((literal, tail)) = remaining.split_once('&') {
            out.push_str(literal);
            let (encoded, tail) = tail.split_once('-').ok_or(ImapDataError)?;
            if encoded.is_empty() {
                out.push('&');
            } else {
                if !encoded
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b','))
                {
                    return Err(ImapDataError);
                }
                let bytes = STANDARD_NO_PAD
                    .decode(encoded.replace(',', "/"))
                    .map_err(|_| ImapDataError)?;
                if bytes.is_empty() || bytes.len() % 2 != 0 {
                    return Err(ImapDataError);
                }
                let units = bytes
                    .chunks_exact(2)
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect::<Vec<_>>();
                let decoded = String::from_utf16(&units).map_err(|_| ImapDataError)?;
                if decoded.chars().any(|c| (' '..='~').contains(&c)) {
                    return Err(ImapDataError);
                }
                out.push_str(&decoded);
            }
            remaining = tail;
        }
        out.push_str(remaining);
        // Reject noncanonical shift forms before folding the special INBOX name.
        if encode_name(&out) != wire {
            return Err(ImapDataError);
        }
        Self::from_unicode(&out)
    }
}
fn encode_name(name: &str) -> String {
    let mut out = String::new();
    let mut unicode = Vec::new();
    for ch in name.chars() {
        if (' '..='~').contains(&ch) {
            encode_run(&mut out, &mut unicode);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            let mut units = [0; 2];
            for unit in ch.encode_utf16(&mut units) {
                unicode.extend_from_slice(&unit.to_be_bytes());
            }
        }
    }
    encode_run(&mut out, &mut unicode);
    out
}
fn encode_run(out: &mut String, bytes: &mut Vec<u8>) {
    if !bytes.is_empty() {
        out.push('&');
        out.push_str(&STANDARD_NO_PAD.encode(&bytes).replace('/', ","));
        out.push('-');
        bytes.clear();
    }
}

pub fn uid_batches(ids: &[NonZeroU32], batch: usize) -> Result<Vec<String>, ImapDataError> {
    if batch == 0 || batch > 1000 || ids.len() > 1_000_000 {
        return Err(ImapDataError);
    }
    let ordered = ids
        .iter()
        .map(|n| n.get())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    Ok(ordered
        .chunks(batch)
        .map(|chunk| {
            let mut ranges = Vec::new();
            let mut rest = chunk;
            while let Some((&start, tail)) = rest.split_first() {
                let mut end = start;
                rest = tail;
                while let Some((&next, tail)) = rest.split_first() {
                    if end.checked_add(1) != Some(next) {
                        break;
                    }
                    end = next;
                    rest = tail;
                }
                ranges.push(if start == end {
                    start.to_string()
                } else {
                    format!("{start}:{end}")
                });
            }
            ranges.join(",")
        })
        .collect())
}
pub fn new_uid_range(highest: u32, uid_next: u32) -> Result<Option<String>, ImapDataError> {
    let end = uid_next.checked_sub(1).ok_or(ImapDataError)?;
    if highest >= end {
        return Ok(None);
    }
    let start = highest.checked_add(1).ok_or(ImapDataError)?;
    Ok(Some(format!("{start}:{end}")))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct ImapFlags(Vec<String>);
impl TryFrom<Vec<String>> for ImapFlags {
    type Error = ImapDataError;
    fn try_from(value: Vec<String>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<ImapFlags> for Vec<String> {
    fn from(value: ImapFlags) -> Self {
        value.0
    }
}
impl ImapFlags {
    pub fn new(flags: Vec<String>) -> Result<Self, ImapDataError> {
        if flags.len() > 128 {
            return Err(ImapDataError);
        }
        let mut values = BTreeSet::new();
        for flag in flags {
            let keyword = flag.strip_prefix('\\').unwrap_or(&flag);
            if flag.len() > 255
                || keyword.is_empty()
                || !keyword
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && !b"(){%*\"\\]".contains(&b))
            {
                return Err(ImapDataError);
            }
            let canonical = match flag.to_ascii_lowercase().as_str() {
                "\\seen" => "\\Seen",
                "\\flagged" => "\\Flagged",
                "\\answered" => "\\Answered",
                "\\deleted" => "\\Deleted",
                "\\draft" => "\\Draft",
                "\\recent" => "\\Recent",
                _ => &flag,
            };
            values.insert(canonical.to_owned());
        }
        Ok(Self(values.into_iter().collect()))
    }
    pub fn values(&self) -> &[String] {
        &self.0
    }
    pub fn seen(&self) -> bool {
        self.0.iter().any(|f| f == "\\Seen")
    }
    pub fn flagged(&self) -> bool {
        self.0.iter().any(|f| f == "\\Flagged")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapMailboxState {
    pub name: MailboxName,
    pub delimiter: Option<char>,
    pub attributes: Vec<String>,
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub highest_mod_seq: Option<u64>,
}
impl ImapMailboxState {
    pub fn validated(mut self) -> Result<Self, ImapDataError> {
        self.attributes = ImapFlags::new(self.attributes)?.into();
        if self
            .delimiter
            .is_some_and(|c| !c.is_ascii() || c.is_control())
            || self.highest_mod_seq == Some(0)
        {
            return Err(ImapDataError);
        }
        let noselect = self
            .attributes
            .iter()
            .any(|a| a.eq_ignore_ascii_case("\\Noselect"));
        if noselect {
            if self.uid_validity.is_some()
                || self.uid_next.is_some()
                || self.highest_mod_seq.is_some()
            {
                return Err(ImapDataError);
            }
        } else if self.uid_validity.is_none_or(|v| v == 0) || self.uid_next.is_none_or(|v| v == 0) {
            return Err(ImapDataError);
        }
        Ok(self)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapMessageState {
    pub placement: ImapPlacement,
    pub flags: ImapFlags,
}
