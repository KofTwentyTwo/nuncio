use super::wire::{Reply, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use mailparse::MailHeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const INITIAL_HISTORY: u64 = 9_007_199_254_740_993;

#[derive(Clone, Serialize, Deserialize)]
pub struct RemoteMessage {
    #[serde(default)]
    pub omitted_fields: BTreeSet<String>,
    pub id: String,
    pub thread_id: String,
    pub raw: Vec<u8>,
    pub labels: BTreeSet<String>,
    pub history_id: u64,
    pub payload: Value,
    pub attachments: BTreeMap<String, Vec<u8>>,
    pub message_id: String,
    pub subject: String,
    pub in_reply_to: String,
    pub references: String,
}
impl RemoteMessage {
    pub(super) fn from_raw(
        id: String,
        thread_id: String,
        raw: Vec<u8>,
        labels: BTreeSet<String>,
        history_id: u64,
    ) -> Result<Self> {
        let parsed =
            mailparse::parse_mail(&raw).map_err(|_| Reply::error(400, "invalidArgument"))?;
        if parsed.headers.get_first_value("From").is_none()
            || parsed.headers.get_first_value("Subject").is_none()
        {
            return Err(Reply::error(400, "invalidArgument"));
        }
        let mut attachments = BTreeMap::new();
        let payload = part(&parsed, "", &mut attachments)?;
        let message_id = parsed
            .headers
            .get_first_value("Message-ID")
            .unwrap_or_default();
        let subject = parsed
            .headers
            .get_first_value("Subject")
            .unwrap_or_default();
        let in_reply_to = parsed
            .headers
            .get_first_value("In-Reply-To")
            .unwrap_or_default();
        let references = parsed
            .headers
            .get_first_value("References")
            .unwrap_or_default();
        Ok(Self {
            omitted_fields: BTreeSet::new(),
            id,
            thread_id,
            raw,
            labels,
            history_id,
            payload,
            attachments,
            message_id,
            subject,
            in_reply_to,
            references,
        })
    }
    pub(super) fn reference(&self) -> Value {
        json!({"id":self.id,"threadId":self.thread_id})
    }
    pub(super) fn wire(&self, format: &str, headers: &[String]) -> Result<Value> {
        let mut value = json!({"id":self.id,"threadId":self.thread_id,"labelIds":self.labels,
            "historyId":self.history_id.to_string(),"internalDate":"1772895600000","sizeEstimate":self.raw.len()});
        match format {
            "raw" => value["raw"] = json!(URL_SAFE_NO_PAD.encode(&self.raw)),
            "full" => value["payload"] = self.payload.clone(),
            "metadata" => {
                let kept: Vec<_> = self.payload["headers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|h| {
                        headers.is_empty()
                            || headers.iter().any(|name| {
                                h["name"]
                                    .as_str()
                                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
                            })
                    })
                    .cloned()
                    .collect();
                value["payload"] = json!({"mimeType":self.payload["mimeType"],"headers":kept});
            }
            "minimal" => {}
            _ => return Err(Reply::error(400, "invalidArgument")),
        }
        if let Some(object) = value.as_object_mut() {
            for field in &self.omitted_fields {
                object.remove(field);
            }
        }
        Ok(value)
    }
}

fn part(
    parsed: &mailparse::ParsedMail<'_>,
    id: &str,
    attachments: &mut BTreeMap<String, Vec<u8>>,
) -> Result<Value> {
    let disposition = parsed.get_content_disposition();
    let filename = disposition
        .params
        .get("filename")
        .or_else(|| parsed.ctype.params.get("name"))
        .cloned()
        .unwrap_or_default();
    let headers: Vec<_> = parsed
        .headers
        .iter()
        .map(|header| json!({"name":header.get_key(),"value":header.get_value()}))
        .collect();
    let mut value = json!({"partId":id,"mimeType":parsed.ctype.mimetype,"filename":filename,"headers":headers,"body":{"size":0}});
    if parsed.subparts.is_empty() {
        let bytes = parsed
            .get_body_raw()
            .map_err(|_| Reply::error(400, "invalidArgument"))?;
        if !filename.is_empty() {
            let attachment = format!("attachment-{id}");
            value["body"] = json!({"size":bytes.len(),"attachmentId":attachment});
            attachments.insert(attachment, bytes);
        } else {
            value["body"] = json!({"size":bytes.len(),"data":URL_SAFE_NO_PAD.encode(bytes)});
        }
    } else {
        let mut parts = Vec::new();
        for (index, child) in parsed.subparts.iter().enumerate() {
            let id = if id.is_empty() {
                index.to_string()
            } else {
                format!("{id}.{index}")
            };
            parts.push(part(child, &id, attachments)?);
        }
        value["parts"] = json!(parts);
    }
    Ok(value)
}

#[derive(Clone, Serialize)]
pub struct AcceptedSend {
    pub id: String,
    pub thread_id: String,
    pub raw: Vec<u8>,
    pub message_id: String,
}

#[derive(Clone, Serialize)]
pub struct MailboxSnapshot {
    pub messages: BTreeMap<String, RemoteMessage>,
    pub history_id: String,
    pub accepted_sends: Vec<AcceptedSend>,
    pub message_copies: u64,
}

pub(super) struct Mailbox {
    pub messages: BTreeMap<String, RemoteMessage>,
    pub labels: BTreeMap<String, Value>,
    pub history: Vec<Value>,
    pub history_id: u64,
    pub history_floor: u64,
    pub sends: Vec<AcceptedSend>,
}
impl Mailbox {
    pub fn seeded(account: &str) -> Result<Self> {
        let labels = [
            "INBOX", "UNREAD", "STARRED", "SENT", "TRASH", "SPAM", "DRAFT",
        ]
        .into_iter()
        .map(|id| (id.into(), json!({"id":id,"name":id,"type":"system"})))
        .chain([(
            "Label_project".into(),
            json!({"id":"Label_project","name":"Project","type":"user"}),
        )])
        .collect();
        let mut messages = BTreeMap::new();
        for (id, thread, bytes, label_ids) in [
            (
                "m-001",
                "t-001",
                include_bytes!("../../fixtures/mime/multipart.eml").as_slice(),
                vec!["INBOX", "UNREAD", "Label_project"],
            ),
            (
                "m-002",
                "t-001",
                include_bytes!("../../fixtures/mime/reply.eml").as_slice(),
                vec!["INBOX"],
            ),
            (
                "m-003",
                "t-002",
                include_bytes!("../../fixtures/mime/html-only.eml").as_slice(),
                vec!["INBOX"],
            ),
        ] {
            let raw = String::from_utf8_lossy(bytes)
                .replace("ACCOUNT", account)
                .replace('\n', "\r\n")
                .into_bytes();
            let message = RemoteMessage::from_raw(
                id.into(),
                thread.into(),
                raw,
                label_ids.into_iter().map(String::from).collect(),
                INITIAL_HISTORY,
            )?;
            messages.insert(id.into(), message);
        }
        Ok(Self {
            messages,
            labels,
            history: Vec::new(),
            history_id: INITIAL_HISTORY,
            history_floor: INITIAL_HISTORY,
            sends: Vec::new(),
        })
    }
    pub fn next_history(&mut self) -> u64 {
        self.history_id += 14;
        self.history_id
    }
    pub fn snapshot(&self) -> MailboxSnapshot {
        MailboxSnapshot {
            messages: self.messages.clone(),
            history_id: self.history_id.to_string(),
            accepted_sends: self.sends.clone(),
            message_copies: 0,
        }
    }
    pub fn change_labels(&mut self, id: &str, add: &[String], remove: &[String]) -> Result<Value> {
        if add
            .iter()
            .chain(remove)
            .any(|label| !self.labels.contains_key(label))
            || add.len() > 100
            || remove.len() > 100
        {
            return Err(Reply::error(400, "invalidArgument"));
        }
        let message = self
            .messages
            .get_mut(id)
            .ok_or_else(|| Reply::error(404, "notFound"))?;
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for label in remove {
            if message.labels.remove(label) {
                removed.push(label.clone());
            }
        }
        for label in add {
            if message.labels.insert(label.clone()) {
                added.push(label.clone());
            }
        }
        let reference = message.reference();
        if !added.is_empty() || !removed.is_empty() {
            self.history_id += 14;
            message.history_id = self.history_id;
            let mut entry = json!({"id":self.history_id.to_string(),"messages":[reference]});
            if !added.is_empty() {
                entry["labelsAdded"] = json!([{"message":reference,"labelIds":added}]);
            }
            if !removed.is_empty() {
                entry["labelsRemoved"] = json!([{"message":reference,"labelIds":removed}]);
            }
            self.history.push(entry);
        }
        message.wire("minimal", &[])
    }
}
