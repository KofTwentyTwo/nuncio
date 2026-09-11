use super::{
    drafts::{DraftContent, DraftError, Recipient},
    mail,
};
use mail_parser::{Address, MimeHeaders};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrepareKind {
    Reply,
    ReplyAll,
    Forward,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftContext {
    pub source_message_id: String,
    pub kind: String,
    pub original_subject: String,
    pub thread_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}
pub struct PreparedAttachment {
    pub filename: Option<String>,
    pub mime_type: String,
    pub parameters: BTreeMap<String, String>,
    pub content_id: Option<String>,
    pub disposition: String,
    pub bytes: Vec<u8>,
}
pub struct PreparedDraft {
    pub content: DraftContent,
    pub context: DraftContext,
    pub attachments: Vec<PreparedAttachment>,
}

pub fn prepare(
    raw: &[u8],
    self_address: &str,
    source: &str,
    thread: Option<&str>,
    kind: PrepareKind,
    body: Option<String>,
    to: Vec<Recipient>,
) -> Result<PreparedDraft, DraftError> {
    if kind != PrepareKind::Forward && !to.is_empty() {
        return Err(DraftError::Recipient);
    }
    let request = DraftContent {
        to,
        text: body,
        ..Default::default()
    };
    request.validate()?;
    mail::with_parsed(raw, mail::MAX_PAYLOAD_BYTES, |message| {
        let mut decoded =
            mail::decode(message, raw, mail::MAX_PAYLOAD_BYTES).map_err(|_| DraftError::Source)?;
        let subject = decoded.subject.take().unwrap_or_default();
        let mut content = request;
        let mut context = DraftContext {
            source_message_id: source.into(),
            original_subject: subject.clone(),
            kind: match kind {
                PrepareKind::Reply => "reply",
                PrepareKind::ReplyAll => "reply_all",
                PrepareKind::Forward => "forward",
            }
            .into(),
            thread_id: None,
            in_reply_to: None,
            references: Vec::new(),
        };
        let mut attachments = Vec::new();
        if kind == PrepareKind::Forward {
            content.subject = prefix(&subject, "Fwd:");
            let intro = format!(
                "{}\n\n---------- Forwarded message ----------\nSubject: {}\n\n",
                content.text.as_deref().unwrap_or_default(),
                subject
            );
            content.html = decoded
                .html
                .map(|html| format!("<pre>{}</pre>{html}", escape(&intro)));
            content.text = Some(format!(
                "{intro}{}",
                decoded.text.unwrap_or(decoded.search_text)
            ));
            if decoded.attachments.len() > 256 {
                return Err(DraftError::TooLarge);
            }
            for attachment in decoded.attachments {
                let part = message
                    .parts
                    .get(attachment.part_index as usize)
                    .ok_or(DraftError::Source)?;
                let mut parameters = BTreeMap::new();
                if let Some(attributes) = part.content_type().and_then(|t| t.attributes.as_ref()) {
                    for attribute in attributes {
                        let name = attribute.name.to_ascii_lowercase();
                        if parameters
                            .insert(name, attribute.value.to_string())
                            .is_some()
                        {
                            return Err(DraftError::Header);
                        }
                    }
                }
                validate_parameters(&parameters)?;
                let disposition = if part
                    .content_disposition()
                    .is_some_and(|d| d.ctype().eq_ignore_ascii_case("inline"))
                {
                    "inline"
                } else {
                    "attachment"
                };
                attachments.push(PreparedAttachment {
                    filename: attachment.filename,
                    mime_type: attachment.mime_type,
                    parameters,
                    content_id: attachment.content_id,
                    disposition: disposition.into(),
                    bytes: attachment.bytes,
                });
            }
        } else {
            content.subject = prefix(&subject, "Re:");
            let mut seen = BTreeSet::from([self_address.to_ascii_lowercase()]);
            add(
                message.reply_to().or_else(|| message.from()),
                &mut content.to,
                &mut seen,
            )?;
            // A reply to one's own sent message addresses the original To list.
            if kind == PrepareKind::ReplyAll || content.to.is_empty() {
                for addresses in message.all_to() {
                    add(Some(addresses), &mut content.to, &mut seen)?;
                }
            }
            if kind == PrepareKind::ReplyAll {
                for addresses in message.all_cc() {
                    add(Some(addresses), &mut content.cc, &mut seen)?;
                }
            }
            context.in_reply_to = message.message_id().map(str::to_owned);
            if let Some(id) = &context.in_reply_to {
                let inherited = message.references().as_text_list().or_else(|| {
                    message
                        .in_reply_to()
                        .as_text_list()
                        .filter(|ids| ids.len() == 1)
                });
                if let Some(ids) = inherited {
                    context.references.extend(ids.iter().map(|s| s.to_string()));
                }
                if context.references.last() != Some(id) {
                    context.references.push(id.clone());
                }
                context.thread_id = thread.map(str::to_owned);
            }
        }
        content.validate()?;
        context.validate()?;
        Ok(PreparedDraft {
            content,
            context,
            attachments,
        })
    })
    .map_err(|_| DraftError::Source)?
}
fn add(
    addresses: Option<&Address<'_>>,
    target: &mut Vec<Recipient>,
    seen: &mut BTreeSet<String>,
) -> Result<(), DraftError> {
    if let Some(addresses) = addresses {
        for address in addresses.iter() {
            let recipient = Recipient {
                address: address
                    .address
                    .as_deref()
                    .ok_or(DraftError::Recipient)?
                    .into(),
                name: address.name.as_deref().map(str::to_owned),
            };
            recipient.validate()?;
            if seen.insert(recipient.address.to_ascii_lowercase()) {
                target.push(recipient);
            }
        }
    }
    Ok(())
}
fn prefix(subject: &str, prefix: &str) -> String {
    if subject
        .get(..prefix.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
    {
        subject.into()
    } else {
        format!("{prefix} {subject}")
    }
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
pub(crate) fn mime_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
pub(crate) fn validate_parameters(parameters: &BTreeMap<String, String>) -> Result<(), DraftError> {
    if parameters.len() > 32
        || parameters
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 8192
        || parameters.iter().any(|(k, v)| {
            !mime_token(k) || k != &k.to_ascii_lowercase() || v.chars().any(char::is_control)
        })
    {
        return Err(DraftError::Header);
    }
    Ok(())
}
impl DraftContext {
    pub fn validate(&self) -> Result<(), DraftError> {
        let valid_id = |id: &str| {
            id.len() <= 998
                && id
                    .split_once('@')
                    .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && !b"<>\"\\()".contains(&b))
        };
        if self.source_message_id.is_empty()
            || self.source_message_id.len() > 128
            || !matches!(self.kind.as_str(), "reply" | "reply_all" | "forward")
            || self.original_subject.len() > 8192
            || self.original_subject.chars().any(char::is_control)
            || self
                .thread_id
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 8192 || s.chars().any(char::is_control))
            || self.references.len() > 100
            || self.references.iter().map(String::len).sum::<usize>() > 16384
            || self.in_reply_to.as_ref().is_some_and(|id| !valid_id(id))
            || self.references.iter().any(|id| !valid_id(id))
        {
            return Err(DraftError::Header);
        }
        Ok(())
    }
}
