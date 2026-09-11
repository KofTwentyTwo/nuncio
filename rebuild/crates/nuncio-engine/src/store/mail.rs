use crate::domain::mail::{DecodedMail, Header};
use serde::{Deserialize, Serialize};
pub const GMAIL_QUERY_FINGERPRINT: &str = "gmail:all:includeSpamTrash=true:v1";
pub struct StagedMail {
    pub provider_id: String,
    pub thread_id: Option<String>,
    pub history_id: Option<String>,
    pub internal_date_ms: Option<i64>,
    pub provider_json: String,
    pub subject: Option<String>,
    pub headers: Vec<Header>,
    pub labels: Vec<String>,
    pub raw: Option<Vec<u8>>,
    pub decoded: Option<DecodedMail>,
    pub availability: String,
}
#[derive(Clone, Serialize)]
pub struct MailCollection {
    pub id: String,
    pub provider_id: String,
    pub name: Option<String>,
    pub kind: String,
    pub retired: bool,
}
#[derive(Clone, Serialize)]
pub struct MailSummary {
    pub id: String,
    pub account_id: String,
    pub provider_id: String,
    pub thread_id: Option<String>,
    pub history_id: Option<String>,
    pub internal_date_ms: Option<i64>,
    pub subject: Option<String>,
    pub body_availability: String,
    pub collections: Vec<MailCollection>,
}
#[derive(Clone, Serialize)]
pub struct MailAttachment {
    pub id: String,
    pub part_index: u32,
    pub filename: Option<String>,
    pub mime_type: String,
    pub content_id: Option<String>,
    pub byte_length: u64,
    pub sha256: String,
}
#[derive(Clone, Serialize)]
pub struct MailDetail {
    pub revision: u64,
    pub message: MailSummary,
    pub headers: Vec<Header>,
    pub attachments: Vec<MailAttachment>,
    pub provider_json: String,
}
#[derive(Clone, Serialize)]
pub struct MailCoverage {
    pub state: String,
    pub synchronized_at_ms: Option<i64>,
    pub cursor: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct MailPage {
    pub revision: u64,
    pub items: Vec<MailSummary>,
    pub next_page_token: Option<String>,
    pub coverage: MailCoverage,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MailQuery {
    pub account_id: String,
    pub collection_id: Option<String>,
    pub query: Option<String>,
    pub page_size: u32,
    pub page_token: Option<String>,
}
