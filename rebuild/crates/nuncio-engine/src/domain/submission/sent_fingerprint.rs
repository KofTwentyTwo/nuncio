use super::DraftError;
use mail_parser::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A versioned content comparison, not a proof that a provider performed a write.
/// SMTP acceptance and a fresh, unique placement observation are separate facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentFingerprint {
    version: u32,
    headers: String,
    body: String,
    bcc: Option<String>,
}
impl SentFingerprint {
    pub fn from_mime(raw: &[u8]) -> Result<Self, DraftError> {
        if raw.len() > crate::domain::mail::MAX_PAYLOAD_BYTES {
            return Err(DraftError::TooLarge);
        }
        let body = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or(DraftError::Header)?
            + 4;
        if body > 1024 * 1024 {
            return Err(DraftError::TooLarge);
        }
        let message = mail_parser::MessageParser::default()
            .parse_headers(raw)
            .ok_or(DraftError::Header)?;
        if message.headers().len() > 512
            || message.message_id().is_none()
            || message.from().is_none()
            || message.date().is_none()
        {
            return Err(DraftError::Header);
        }
        let value = |name: HeaderName<'static>| -> Result<Option<&HeaderValue<'_>>, DraftError> {
            let mut values = message.headers().iter().filter(|h| h.name == name);
            let first = values.next().map(|h| &h.value);
            if values.next().is_some() {
                return Err(DraftError::Header);
            }
            Ok(first)
        };
        let mut headers = Vec::new();
        // Compare parsed values so valid folding/encoded header forms remain
        // equivalent. Trace headers are deliberately outside message identity.
        for name in [
            HeaderName::MessageId,
            HeaderName::From,
            HeaderName::Sender,
            HeaderName::ReplyTo,
            HeaderName::To,
            HeaderName::Cc,
            HeaderName::Date,
            HeaderName::Subject,
            HeaderName::InReplyTo,
            HeaderName::References,
            HeaderName::MimeVersion,
            HeaderName::ContentType,
            HeaderName::ContentTransferEncoding,
        ] {
            headers.push(value(name)?);
        }
        let headers = serde_json::to_vec(&headers).map_err(|_| DraftError::Header)?;
        let bcc = value(HeaderName::Bcc)?
            .map(|v| {
                serde_json::to_vec(v)
                    .map(|bytes| hex::encode(Sha256::digest(bytes)))
                    .map_err(|_| DraftError::Header)
            })
            .transpose()?;
        Ok(Self {
            version: 1,
            headers: hex::encode(Sha256::digest(headers)),
            body: hex::encode(Sha256::digest(&raw[body..])),
            bcc,
        })
    }
    pub fn matches(&self, observed: &Self) -> bool {
        let valid = |value: &Self| {
            value.version == 1
                && std::iter::once(&value.headers)
                    .chain(std::iter::once(&value.body))
                    .chain(value.bcc.iter())
                    .all(|hash| {
                        hash.len() == 64
                            && hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
        };
        valid(self)
            && valid(observed)
            && self.headers == observed.headers
            && self.body == observed.body
            && (observed.bcc.is_none() || self.bcc == observed.bcc)
    }
}
