use mail_parser::{Encoding, MessageParser, MimeHeaders, PartType};
use serde::{Deserialize, Serialize};

pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const BLOB_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("Mail content exceeds the configured payload limit")]
    TooLarge,
    #[error("Original mail content could not be decoded")]
    InvalidMime,
    #[error("Mail structure exceeds the supported part or header count")]
    TooComplex,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}
pub struct DecodedAttachment {
    pub part_index: u32,
    pub filename: Option<String>,
    pub mime_type: String,
    pub content_id: Option<String>,
    pub bytes: Vec<u8>,
}
pub struct DecodedMail {
    pub subject: Option<String>,
    pub headers: Vec<Header>,
    /// Original decoded text/plain content; absent for HTML-only messages.
    pub text: Option<String>,
    /// Original decoded HTML; stored inertly and never loaded into a browser.
    pub html: Option<String>,
    pub search_text: String,
    pub attachments: Vec<DecodedAttachment>,
}

/// Called on the bounded blocking ingestion worker, not on a Tokio reactor.
/// The caller retains the input bytes as the authoritative downloadable original.
pub fn decode_mime(raw: &[u8], limit: usize) -> Result<DecodedMail, MailError> {
    with_parsed(raw, limit, |message| decode(message, raw, limit))?
}
pub(crate) fn with_parsed<T>(
    raw: &[u8],
    limit: usize,
    inspect: impl FnOnce(&mail_parser::Message<'_>) -> T,
) -> Result<T, MailError> {
    if limit == 0 || limit > MAX_PAYLOAD_BYTES || raw.len() > limit {
        return Err(MailError::TooLarge);
    }
    let message = MessageParser::default()
        .parse(raw)
        .ok_or(MailError::InvalidMime)?;
    let result = inspect(&message);
    // Nested unencoded message/rfc822 structures use recursive value types. Drain
    // them iteratively so dropping a parsed attachment tree does not recurse.
    let mut pending = vec![message];
    while let Some(mut message) = pending.pop() {
        for part in message.parts.drain(..) {
            if let PartType::Message(nested) = part.body {
                pending.push(nested);
            }
        }
    }
    Ok(result)
}
pub(crate) fn decode(
    message: &mail_parser::Message<'_>,
    raw: &[u8],
    limit: usize,
) -> Result<DecodedMail, MailError> {
    if message.parts.is_empty() {
        return Err(MailError::InvalidMime);
    }
    if message.parts.len() > 4096 || message.headers().len() > 4096 {
        return Err(MailError::TooComplex);
    }
    let mut headers = Vec::new();
    for header in message.headers() {
        let bytes = raw
            .get(header.offset_start as usize..header.offset_end as usize)
            .ok_or(MailError::InvalidMime)?;
        headers.push(Header {
            name: header.name.as_str().into(),
            value: String::from_utf8_lossy(bytes).trim().into(),
        });
    }
    let mut text = Vec::new();
    let mut html = Vec::new();
    let mut search = Vec::new();
    let mut decoded_bytes = 0;
    for id in &message.text_body {
        let part = message
            .parts
            .get(*id as usize)
            .ok_or(MailError::InvalidMime)?;
        if part.is_encoding_problem {
            return Err(MailError::InvalidMime);
        }
        if let PartType::Text(value) = &part.body {
            add_size(&mut decoded_bytes, value.len(), limit)?;
            text.push(value.as_ref());
        }
    }
    for id in &message.html_body {
        let part = message
            .parts
            .get(*id as usize)
            .ok_or(MailError::InvalidMime)?;
        if part.is_encoding_problem {
            return Err(MailError::InvalidMime);
        }
        if let PartType::Html(value) = &part.body {
            add_size(&mut decoded_bytes, value.len(), limit)?;
            html.push(value.as_ref());
        }
    }
    for index in 0..message.text_body.len() {
        if let Some(value) = message.body_text(index) {
            search.push(value.into_owned());
        }
    }
    let mut attachments = Vec::new();
    for id in &message.attachments {
        let part = message
            .parts
            .get(*id as usize)
            .ok_or(MailError::InvalidMime)?;
        if part.is_encoding_problem {
            return Err(MailError::InvalidMime);
        }
        let encoded = raw
            .get(part.offset_body as usize..part.offset_end as usize)
            .ok_or(MailError::InvalidMime)?;
        // Decode transfer encoding only. contents() converts text attachments to
        // UTF-8, which would change a downloaded ISO-8859-1/UTF-16 attachment.
        let bytes = match part.encoding {
            Encoding::None => encoded.to_vec(),
            Encoding::Base64 => mail_parser::decoders::base64::base64_decode(encoded)
                .ok_or(MailError::InvalidMime)?,
            Encoding::QuotedPrintable => {
                mail_parser::decoders::quoted_printable::quoted_printable_decode(encoded)
                    .ok_or(MailError::InvalidMime)?
            }
        };
        add_size(&mut decoded_bytes, bytes.len(), limit)?;
        let mime_type = part
            .content_type()
            .map(|value| match value.subtype() {
                Some(subtype) => format!("{}/{subtype}", value.ctype()),
                None => value.ctype().into(),
            })
            .unwrap_or_else(|| "application/octet-stream".into());
        attachments.push(DecodedAttachment {
            part_index: *id,
            filename: part.attachment_name().map(str::to_owned),
            mime_type,
            content_id: part.content_id().map(str::to_owned),
            bytes,
        });
    }
    let search_text = search.join("\n");
    if search_text.len() > limit {
        return Err(MailError::TooLarge);
    }
    Ok(DecodedMail {
        subject: message.subject().map(str::to_owned),
        headers,
        text: (!text.is_empty()).then(|| text.join("\n")),
        html: (!html.is_empty()).then(|| html.join("\n")),
        search_text,
        attachments,
    })
}
fn add_size(total: &mut usize, size: usize, limit: usize) -> Result<(), MailError> {
    if size > limit.saturating_sub(*total) {
        return Err(MailError::TooLarge);
    }
    *total += size;
    Ok(())
}
