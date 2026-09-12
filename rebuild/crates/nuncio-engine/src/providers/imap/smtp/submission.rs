use super::{command, response, MailError, Session, Wire};
use crate::domain::{drafts::Recipient, mail::MAX_PAYLOAD_BYTES};
use std::collections::BTreeSet;
use tokio::io::AsyncWriteExt;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SubmissionError {
    #[error("SMTP server rejected the transaction with code {0}")]
    Rejected(u16),
    #[error(transparent)]
    Wire(#[from] MailError),
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DataResult {
    Accepted(u16),
    Rejected(u16),
    Unknown,
}
pub(crate) struct Prepared<'a> {
    wire: Wire,
    raw: &'a [u8],
}
impl Session {
    /// Establish the envelope and receive DATA readiness without sending message
    /// content. The caller commits its dispatch marker before transmit().
    pub(crate) async fn prepare<'a>(
        mut self,
        sender: &str,
        recipients: &[String],
        raw: &'a [u8],
    ) -> Result<Prepared<'a>, SubmissionError> {
        if recipients.is_empty() || recipients.len() > 1000 {
            return Err(MailError::Invalid.into());
        }
        for address in std::iter::once(sender).chain(recipients.iter().map(String::as_str)) {
            Recipient {
                address: address.into(),
                name: None,
            }
            .validate()
            .map_err(|_| MailError::Invalid)?;
        }
        let international = !sender.is_ascii() || recipients.iter().any(|r| !r.is_ascii());
        let (utf8, eight_bit) = validate_data(raw, &self.caps, international)?;
        let request = format!(
            "MAIL FROM:<{sender}>{}{}\r\n",
            if utf8 { " SMTPUTF8" } else { "" },
            if eight_bit { " BODY=8BITMIME" } else { "" }
        );
        positive(command(&mut self.wire, request.as_bytes()).await?)?;
        for recipient in recipients {
            positive(
                command(
                    &mut self.wire,
                    format!("RCPT TO:<{recipient}>\r\n").as_bytes(),
                )
                .await?,
            )?;
        }
        let ready = command(&mut self.wire, b"DATA\r\n").await?;
        if !ready.has_code(354) {
            return Err(rejected(ready));
        }
        Ok(Prepared {
            wire: self.wire,
            raw,
        })
    }
}
impl Prepared<'_> {
    /// Once called, only a complete final reply proves acceptance or rejection.
    /// An interrupted write/read leaves outcome uncertainty for the journal.
    pub(crate) async fn transmit(mut self) -> DataResult {
        let Ok(_request) = self.wire.resources.request().await else {
            return DataResult::Unknown;
        };
        if write_data(&mut self.wire, self.raw).await.is_err() {
            return DataResult::Unknown;
        }
        match response(&mut self.wire).await {
            Ok(reply) => match u16::from(reply.code()) {
                code @ 200..=299 => DataResult::Accepted(code),
                code @ 400..=599 => DataResult::Rejected(code),
                _ => DataResult::Unknown,
            },
            Err(_) => DataResult::Unknown,
        }
    }
}
fn positive(reply: lettre::transport::smtp::response::Response) -> Result<(), SubmissionError> {
    if (200..300).contains(&u16::from(reply.code())) {
        Ok(())
    } else {
        Err(rejected(reply))
    }
}
fn rejected(reply: lettre::transport::smtp::response::Response) -> SubmissionError {
    let code = u16::from(reply.code());
    if (400..600).contains(&code) {
        SubmissionError::Rejected(code)
    } else {
        SubmissionError::Wire(MailError::Protocol)
    }
}

pub(super) fn validate_data(
    raw: &[u8],
    caps: &BTreeSet<String>,
    international_envelope: bool,
) -> Result<(bool, bool), MailError> {
    if raw.is_empty()
        || raw.len() > MAX_PAYLOAD_BYTES
        || !raw.ends_with(b"\r\n")
        || raw.contains(&0)
    {
        return Err(MailError::Invalid);
    }
    for line in raw.split_inclusive(|b| *b == b'\n') {
        let content = line.strip_suffix(b"\r\n").ok_or(MailError::Invalid)?;
        if line.len() > 1000 || content.contains(&b'\r') {
            return Err(MailError::Invalid);
        }
    }
    let boundary = raw
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or(MailError::Invalid)?;
    let parsed = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or(MailError::Invalid)?;
    let mut pending = vec![&parsed];
    let mut parts = 0;
    let mut utf8 = international_envelope;
    while let Some(message) = pending.pop() {
        for part in &message.parts {
            parts += 1;
            if parts > 4096 {
                return Err(MailError::Invalid);
            }
            for header in &part.headers {
                let bytes = message
                    .raw_message()
                    .get(header.offset_field as usize..header.offset_end as usize)
                    .ok_or(MailError::Protocol)?;
                utf8 |= !bytes.is_ascii();
            }
            if let mail_parser::PartType::Message(nested) = &part.body {
                pending.push(nested);
            }
        }
    }
    let eight_bit = !raw[boundary + 4..].is_ascii();
    if (utf8 && !caps.contains("SMTPUTF8")) || (eight_bit && !caps.contains("8BITMIME")) {
        return Err(MailError::Unsupported);
    }
    Ok((utf8, eight_bit))
}
async fn write_data(wire: &mut Wire, raw: &[u8]) -> Result<(), MailError> {
    let mut output = Vec::with_capacity(65536);
    let mut beginning = true;
    for byte in raw {
        if beginning && *byte == b'.' {
            output.push(b'.');
        }
        output.push(*byte);
        beginning = *byte == b'\n';
        if output.len() >= 65534 {
            wire.write_all(&output)
                .await
                .map_err(|_| MailError::Unavailable)?;
            output.clear();
        }
    }
    wire.write_all(&output)
        .await
        .map_err(|_| MailError::Unavailable)?;
    wire.write_all(b".\r\n")
        .await
        .map_err(|_| MailError::Unavailable)?;
    wire.flush().await.map_err(|_| MailError::Unavailable)?;
    Ok(())
}
