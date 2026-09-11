use serde::{Deserialize, Serialize};

pub const MAX_DRAFT_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_DRAFT_RECIPIENTS: usize = 1000;

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("Original message is unavailable or cannot be safely prepared")]
    Source,
    #[error("Invalid recipient address")]
    Recipient,
    #[error("Invalid mail header")]
    Header,
    #[error("Draft exceeds the supported size or recipient count")]
    TooLarge,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Recipient {
    pub address: String,
    #[serde(default)]
    pub name: Option<String>,
}
impl Recipient {
    pub fn validate(&self) -> Result<(), DraftError> {
        if self.address.len() > 320
            || self.address.trim() != self.address
            || self.address.chars().any(char::is_control)
            || email_address::EmailAddress::parse_with_options(
                &self.address,
                email_address::Options {
                    allow_display_text: false,
                    ..Default::default()
                },
            )
            .is_err()
        {
            return Err(DraftError::Recipient);
        }
        if self
            .name
            .as_ref()
            .is_some_and(|name| name.len() > 256 || name.chars().any(char::is_control))
        {
            return Err(DraftError::Header);
        }
        Ok(())
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DraftContent {
    pub to: Vec<Recipient>,
    pub cc: Vec<Recipient>,
    pub bcc: Vec<Recipient>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
}
impl DraftContent {
    pub fn validate(&self) -> Result<(), DraftError> {
        let recipients = self
            .to
            .len()
            .saturating_add(self.cc.len())
            .saturating_add(self.bcc.len());
        let body = self
            .text
            .as_ref()
            .map_or(0, String::len)
            .saturating_add(self.html.as_ref().map_or(0, String::len));
        if recipients > MAX_DRAFT_RECIPIENTS
            || body > MAX_DRAFT_BODY_BYTES
            || self.subject.len() > 8192
        {
            return Err(DraftError::TooLarge);
        }
        if self.subject.chars().any(char::is_control) {
            return Err(DraftError::Header);
        }
        for recipient in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            recipient.validate()?;
        }
        Ok(())
    }
}
