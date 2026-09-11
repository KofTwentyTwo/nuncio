use super::{
    drafts::{DraftContent, DraftError, Recipient},
    mail::MAX_PAYLOAD_BYTES,
    prepare::{mime_token, validate_parameters, DraftContext, PreparedAttachment},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mail_builder::{
    headers::{address::Address, content_type::ContentType},
    mime::MimePart,
    MessageBuilder,
};
use std::{collections::BTreeSet, io::Write};
mod sent_fingerprint;
pub use sent_fingerprint::SentFingerprint;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SubmissionTransport {
    Gmail,
    Smtp,
}
#[derive(Clone)]
pub struct FrozenMessage {
    pub wire: Vec<u8>,
    /// SMTP retains Bcc locally, but never transmits that header to recipients.
    /// Gmail needs Bcc in its submitted MIME to supply envelope recipients.
    pub sent_copy: Option<Vec<u8>>,
    pub recipients: Vec<String>,
    pub thread_id: Option<String>,
    pub message_id: String,
}

pub fn freeze(
    sender: &str,
    content: &DraftContent,
    context: Option<&DraftContext>,
    attachments: &[PreparedAttachment],
    message_id: &str,
    now_ms: i64,
    transport: SubmissionTransport,
) -> Result<FrozenMessage, DraftError> {
    content.validate()?;
    Recipient {
        address: sender.into(),
        name: None,
    }
    .validate()?;
    validate_identity(message_id, now_ms)?;
    let mut seen = BTreeSet::new();
    let recipients = content
        .to
        .iter()
        .chain(&content.cc)
        .chain(&content.bcc)
        .filter(|r| seen.insert(r.address.to_ascii_lowercase()))
        .map(|r| r.address.clone())
        .collect::<Vec<_>>();
    if recipients.is_empty() {
        return Err(DraftError::Recipient);
    }
    if attachments.len() > 256
        || attachments.iter().map(|a| a.bytes.len()).sum::<usize>() > MAX_PAYLOAD_BYTES
    {
        return Err(DraftError::TooLarge);
    }
    let mut builder = MessageBuilder::new()
        .from(sender)
        .subject(content.subject.as_str())
        .message_id(message_id)
        .date(now_ms / 1000);
    if !content.to.is_empty() {
        builder = builder.to(addresses(&content.to));
    }
    if !content.cc.is_empty() {
        builder = builder.cc(addresses(&content.cc));
    }
    if !content.bcc.is_empty() {
        builder = builder.bcc(addresses(&content.bcc));
    }
    let mut thread_id = None;
    if let Some(context) = context {
        context.validate()?;
        if context.kind != "forward" {
            if let Some(id) = context.in_reply_to.as_deref() {
                builder = builder.in_reply_to(id);
            }
            if !context.references.is_empty() {
                builder = builder.references(
                    context
                        .references
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                );
            }
            if subject_key(&content.subject) == subject_key(&context.original_subject) {
                thread_id = context.thread_id.clone();
            }
        }
    }
    let mut inline = Vec::new();
    let mut ordinary = Vec::new();
    for attachment in attachments {
        let part = attachment_part(attachment)?;
        if attachment.disposition == "inline" && content.html.is_some() {
            inline.push(part);
        } else {
            ordinary.push(part);
        }
    }
    let html = content.html.as_deref().map(|html| {
        let html = MimePart::new("text/html", html);
        if inline.is_empty() {
            html
        } else {
            let mut related = vec![html];
            related.extend(inline);
            MimePart::new("multipart/related", related)
        }
    });
    let body = match (content.text.as_deref(), html) {
        (Some(text), Some(html)) => MimePart::new(
            "multipart/alternative",
            vec![MimePart::new("text/plain", text), html],
        ),
        (Some(text), None) => MimePart::new("text/plain", text),
        (None, Some(html)) => html,
        (None, None) => MimePart::new("text/plain", ""),
    };
    let body = if ordinary.is_empty() {
        body
    } else {
        let mut parts = vec![body];
        parts.extend(ordinary);
        MimePart::new("multipart/mixed", parts)
    };
    let mut output = LimitedBytes::default();
    builder
        .body(body)
        .write_to(&mut output)
        .map_err(|_| DraftError::TooLarge)?;
    // The DATA terminator follows a complete CRLF line. Freeze this byte now
    // so SMTP transport and the independently retained Sent copy stay identical.
    if transport == SubmissionTransport::Smtp && !output.0.ends_with(b"\r\n") {
        output
            .write_all(b"\r\n")
            .map_err(|_| DraftError::TooLarge)?;
    }
    let (wire, sent_copy) = if transport == SubmissionTransport::Smtp && !content.bcc.is_empty() {
        let wire = without_bcc(&output.0)?;
        (wire, Some(output.0))
    } else {
        (output.0, None)
    };
    Ok(FrozenMessage {
        wire,
        sent_copy,
        recipients,
        thread_id,
        message_id: message_id.into(),
    })
}
fn validate_identity(message_id: &str, now_ms: i64) -> Result<(), DraftError> {
    if now_ms < 0
        || now_ms / 1000 > 253402300799
        || message_id.len() > 998
        || !message_id
            .split_once('@')
            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
        || !message_id
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"<>\"\\()".contains(&b))
    {
        return Err(DraftError::Header);
    }
    Ok(())
}

/// A deliberate new submission changes only its outer identity and timestamp.
/// The frozen body, including any nested messages, remains byte-for-byte intact.
pub fn reidentify(original: &[u8], message_id: &str, now_ms: i64) -> Result<Vec<u8>, DraftError> {
    use mail_builder::headers::{date::Date, message_id::MessageId, Header};
    use mail_parser::HeaderName;
    validate_identity(message_id, now_ms)?;
    if original.len() > MAX_PAYLOAD_BYTES {
        return Err(DraftError::TooLarge);
    }
    let parsed = mail_parser::MessageParser::default()
        .parse_headers(original)
        .ok_or(DraftError::Source)?;
    for name in [HeaderName::MessageId, HeaderName::Date] {
        if parsed.headers().iter().filter(|h| h.name == name).count() != 1 {
            return Err(DraftError::Source);
        }
    }
    let mut output = LimitedBytes::default();
    let write = |result: std::io::Result<()>| result.map_err(|_| DraftError::TooLarge);
    write(output.write_all(b"Message-ID: "))?;
    MessageId::new(message_id)
        .write_header(&mut output, 12)
        .map_err(|_| DraftError::TooLarge)?;
    write(output.write_all(b"Date: "))?;
    Date::new(now_ms / 1000)
        .write_header(&mut output, 6)
        .map_err(|_| DraftError::TooLarge)?;
    let mut offset = 0;
    for header in parsed
        .headers()
        .iter()
        .filter(|h| matches!(h.name, HeaderName::MessageId | HeaderName::Date))
    {
        let start = header.offset_field as usize;
        let end = header.offset_end as usize;
        write(output.write_all(original.get(offset..start).ok_or(DraftError::Source)?))?;
        offset = end;
    }
    write(output.write_all(original.get(offset..).ok_or(DraftError::Source)?))?;
    Ok(output.0)
}
fn addresses(recipients: &[Recipient]) -> Address<'_> {
    Address::new_list(
        recipients
            .iter()
            .map(|r| Address::new_address(r.name.as_deref(), r.address.as_str()))
            .collect(),
    )
}
fn subject_key(mut subject: &str) -> &str {
    subject = subject.trim();
    while subject
        .get(..3)
        .is_some_and(|s| s.eq_ignore_ascii_case("re:"))
    {
        subject = subject[3..].trim_start();
    }
    subject
}
fn attachment_part(a: &PreparedAttachment) -> Result<MimePart<'_>, DraftError> {
    if !a
        .mime_type
        .split_once('/')
        .is_some_and(|(a, b)| mime_token(a) && mime_token(b))
        || a.mime_type.len() > 255
        || !matches!(a.disposition.as_str(), "attachment" | "inline")
        || a.filename
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 1024 || s.chars().any(char::is_control))
        || a.content_id.as_ref().is_some_and(|s| {
            s.is_empty()
                || s.len() > 255
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && !b"<>\"\\()[];,:".contains(&b))
        })
    {
        return Err(DraftError::Header);
    }
    validate_parameters(&a.parameters)?;
    let mut content_type = ContentType::new(a.mime_type.as_str());
    for (key, value) in &a.parameters {
        content_type = content_type.attribute(key.as_str(), value.as_str());
    }
    let mut disposition = ContentType::new(a.disposition.as_str());
    if let Some(filename) = a.filename.as_deref() {
        disposition = disposition.attribute("filename", filename);
    }
    let mut part = if a.mime_type.eq_ignore_ascii_case("message/rfc822")
        || a.mime_type.to_ascii_lowercase().starts_with("multipart/")
    {
        // MIME forbids base64 for these container types. Preserve the original
        // nested bytes; the SMTP executor must negotiate their transport needs.
        MimePart::new(content_type, a.bytes.as_slice()).transfer_encoding("8bit")
    } else {
        // Treat every leaf attachment as opaque bytes, including inline text.
        // Explicit base64 avoids body newline/charset normalization by encoders.
        let mut encoded = LimitedBytes::default();
        for chunk in a.bytes.chunks(57) {
            encoded
                .write_all(STANDARD.encode(chunk).as_bytes())
                .map_err(|_| DraftError::TooLarge)?;
            encoded
                .write_all(b"\r\n")
                .map_err(|_| DraftError::TooLarge)?;
        }
        MimePart::new(content_type, encoded.0).transfer_encoding("base64")
    }
    .header("Content-Disposition", disposition);
    if let Some(id) = a.content_id.as_deref() {
        part = part.cid(id);
    }
    Ok(part)
}
fn without_bcc(original: &[u8]) -> Result<Vec<u8>, DraftError> {
    let headers = mail_parser::MessageParser::default()
        .parse_headers(original)
        .ok_or(DraftError::Source)?;
    let mut output = Vec::with_capacity(original.len());
    let mut offset = 0;
    for header in headers
        .headers()
        .iter()
        .filter(|h| h.name == mail_parser::HeaderName::Bcc)
    {
        let start = header.offset_field as usize;
        let end = header.offset_end as usize;
        output.extend_from_slice(original.get(offset..start).ok_or(DraftError::Source)?);
        offset = end;
    }
    output.extend_from_slice(original.get(offset..).ok_or(DraftError::Source)?);
    Ok(output)
}
#[derive(Default)]
struct LimitedBytes(Vec<u8>);
impl Write for LimitedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_PAYLOAD_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("MIME exceeds payload limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
