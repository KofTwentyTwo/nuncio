use super::{
    mail_state::{AcceptedSend, RemoteMessage},
    paging,
    state::Model,
    wire::{Input, Reply, Result},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use mailparse::MailHeaderMap;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Modify {
    #[serde(default)]
    add_label_ids: Vec<String>,
    #[serde(default)]
    remove_label_ids: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Send {
    raw: String,
    #[serde(default)]
    thread_id: Option<String>,
}

pub(super) fn route(model: &mut Model, input: &Input, account: &str) -> Result<Reply> {
    let decoded = input.segments()?;
    let segments: Vec<_> = decoded.iter().map(String::as_str).collect();
    if segments.len() < 5 || segments[..3] != ["gmail", "v1", "users"] {
        return Err(Reply::error(404, "notFound"));
    }
    if segments[3] != "me" && segments[3] != account {
        return Err(Reply::error(403, "forbidden"));
    }
    if input.method == "GET" {
        match &segments[4..] {
            ["profile"] => {
                input.query.validate(&[], &[])?;
                let mailbox = model.mailbox(account)?;
                Ok(Reply::json(
                    json!({"emailAddress":account,"messagesTotal":mailbox.messages.len(),
                    "threadsTotal":mailbox.messages.values().map(|m|&m.thread_id).collect::<BTreeSet<_>>().len(),"historyId":mailbox.history_id.to_string()}),
                ))
            }
            ["labels"] => {
                input.query.validate(&[], &[])?;
                Ok(Reply::json(
                    json!({"labels":model.mailbox(account)?.labels.values().collect::<Vec<_>>()}),
                ))
            }
            ["messages"] => list_messages(model, input, account),
            ["history"] => list_history(model, input, account),
            ["messages", id] => {
                input
                    .query
                    .validate(&["format", "metadataHeaders"], &["metadataHeaders"])?;
                let headers = input
                    .query
                    .0
                    .get("metadataHeaders")
                    .cloned()
                    .unwrap_or_default();
                Ok(Reply::json(
                    model
                        .mailbox(account)?
                        .messages
                        .get(*id)
                        .ok_or_else(|| Reply::error(404, "notFound"))?
                        .wire(input.query.get("format").unwrap_or("full"), &headers)?,
                ))
            }
            ["messages", id, "attachments", attachment] => {
                input.query.validate(&[], &[])?;
                let bytes = model
                    .mailbox(account)?
                    .messages
                    .get(*id)
                    .and_then(|m| m.attachments.get(*attachment))
                    .ok_or_else(|| Reply::error(404, "notFound"))?;
                Ok(Reply::json(
                    json!({"size":bytes.len(),"data":URL_SAFE_NO_PAD.encode(bytes)}),
                ))
            }
            ["threads", id] => {
                input
                    .query
                    .validate(&["format", "metadataHeaders"], &["metadataHeaders"])?;
                let format = input.query.get("format").unwrap_or("full");
                if !["full", "metadata", "minimal"].contains(&format) {
                    return Err(Reply::error(400, "invalidArgument"));
                }
                let headers = input
                    .query
                    .0
                    .get("metadataHeaders")
                    .cloned()
                    .unwrap_or_default();
                let mailbox = model.mailbox(account)?;
                let messages = mailbox
                    .messages
                    .values()
                    .filter(|m| m.thread_id == *id)
                    .map(|m| m.wire(format, &headers))
                    .collect::<Result<Vec<_>>>()?;
                if messages.is_empty() {
                    return Err(Reply::error(404, "notFound"));
                }
                Ok(Reply::json(
                    json!({"id":id,"historyId":mailbox.history_id.to_string(),"messages":messages}),
                ))
            }
            _ => Err(Reply::error(404, "notFound")),
        }
    } else if input.method == "POST" {
        input.query.validate(&[], &[])?;
        match &segments[4..] {
            ["messages", "send"] => send(model, input, account),
            ["messages", id, "modify"] => {
                let change: Modify = input.json()?;
                Ok(Reply::json(model.mailbox_mut(account)?.change_labels(
                    id,
                    &change.add_label_ids,
                    &change.remove_label_ids,
                )?))
            }
            ["messages", id, action @ ("trash" | "untrash")] => {
                if !input.body.is_empty() && input.json::<Value>()? != json!({}) {
                    return Err(Reply::error(400, "invalidArgument"));
                }
                let (add, remove) = if *action == "trash" {
                    (vec!["TRASH".into()], vec!["INBOX".into(), "SPAM".into()])
                } else {
                    (vec!["INBOX".into()], vec!["TRASH".into()])
                };
                Ok(Reply::json(
                    model
                        .mailbox_mut(account)?
                        .change_labels(id, &add, &remove)?,
                ))
            }
            _ => Err(Reply::error(404, "notFound")),
        }
    } else {
        Err(Reply::error(405, "methodNotAllowed"))
    }
}

fn list_messages(model: &mut Model, input: &Input, account: &str) -> Result<Reply> {
    input.query.validate(
        &[
            "maxResults",
            "pageToken",
            "labelIds",
            "includeSpamTrash",
            "q",
        ],
        &["labelIds"],
    )?;
    let size = input.query.page_size(100, 500)?;
    let spam = input.query.boolean("includeSpamTrash", false)?;
    let search = input
        .query
        .get("q")
        .map(|q| {
            q.strip_prefix("rfc822msgid:")
                .ok_or_else(|| Reply::error(400, "unsupportedQuery"))
        })
        .transpose()?;
    let scope = paging::scope(account, input);
    if let Some(reply) = model.saved_page(&scope, input)? {
        return Ok(reply);
    }
    let labels = input.query.0.get("labelIds").cloned().unwrap_or_default();
    let mailbox = model.mailbox(account)?;
    let mut messages: Vec<_> = mailbox
        .messages
        .values()
        .filter(|m| {
            (spam || !m.labels.contains("TRASH") && !m.labels.contains("SPAM"))
                && labels.iter().all(|label| m.labels.contains(label))
                && search.is_none_or(|needle| {
                    m.message_id.trim_matches(['<', '>']) == needle.trim_matches(['<', '>'])
                })
        })
        .map(RemoteMessage::reference)
        .collect();
    if model.reverse {
        messages.reverse();
    }
    let total = messages.len();
    Ok(model.paginate(
        scope,
        "messages",
        messages,
        size,
        json!({"resultSizeEstimate":total}),
        json!({}),
    ))
}

fn list_history(model: &mut Model, input: &Input, account: &str) -> Result<Reply> {
    input.query.validate(
        &[
            "maxResults",
            "pageToken",
            "startHistoryId",
            "historyTypes",
            "labelId",
        ],
        &["historyTypes"],
    )?;
    let size = input.query.page_size(100, 500)?;
    let start = input
        .query
        .required("startHistoryId")?
        .parse::<u64>()
        .map_err(|_| Reply::error(404, "notFound"))?;
    let mailbox = model.mailbox(account)?;
    if start < mailbox.history_floor
        || start > mailbox.history_id
        || (start != super::mail_state::INITIAL_HISTORY
            && start != mailbox.history_id
            && !mailbox.history.iter().any(|entry| {
                entry["id"].as_str().and_then(|s| s.parse::<u64>().ok()) == Some(start)
            }))
    {
        return Err(Reply::error(404, "notFound"));
    }
    let scope = paging::scope(account, input);
    if let Some(reply) = model.saved_page(&scope, input)? {
        return Ok(reply);
    }
    let types = input
        .query
        .0
        .get("historyTypes")
        .cloned()
        .unwrap_or_default();
    let mapping = [
        ("messageAdded", "messagesAdded"),
        ("messageDeleted", "messagesDeleted"),
        ("labelAdded", "labelsAdded"),
        ("labelRemoved", "labelsRemoved"),
    ];
    if types
        .iter()
        .any(|t| !mapping.iter().any(|(name, _)| name == t))
    {
        return Err(Reply::error(400, "invalidArgument"));
    }
    let history = mailbox
        .history
        .iter()
        .filter(|entry| {
            entry["id"]
                .as_str()
                .and_then(|id| id.parse::<u64>().ok())
                .is_some_and(|id| id > start)
        })
        .filter(|entry| {
            types.is_empty()
                || mapping.iter().any(|(name, field)| {
                    types.iter().any(|t| t == name) && entry.get(field).is_some()
                })
        })
        .filter(|entry| {
            input.query.get("labelId").is_none_or(|label| {
                entry["messages"].as_array().into_iter().flatten().any(|m| {
                    m["id"]
                        .as_str()
                        .and_then(|id| mailbox.messages.get(id))
                        .is_some_and(|m| m.labels.contains(label))
                })
            })
        })
        .cloned()
        .collect();
    let final_history = mailbox.history_id.to_string();
    Ok(model.paginate(
        scope,
        "history",
        history,
        size,
        json!({"historyId":final_history}),
        json!({}),
    ))
}

fn send(model: &mut Model, input: &Input, account: &str) -> Result<Reply> {
    let request: Send = input.json()?;
    let raw = URL_SAFE_NO_PAD
        .decode(&request.raw)
        .map_err(|_| Reply::error(400, "invalidArgument"))?;
    let parsed = mailparse::parse_mail(&raw).map_err(|_| Reply::error(400, "invalidArgument"))?;
    let mut recipients = 0;
    for header in ["To", "Cc", "Bcc"] {
        if let Some(value) = parsed.headers.get_first_value(header) {
            recipients += mailparse::addrparse(&value)
                .map_err(|_| Reply::error(400, "invalidRecipient"))?
                .count_addrs();
        }
    }
    if recipients == 0 {
        return Err(Reply::error(400, "recipientRequired"));
    }
    let id = model.next("sent");
    let thread = request
        .thread_id
        .clone()
        .unwrap_or_else(|| model.next("thread"));
    let mailbox = model.mailbox_mut(account)?;
    let mut message = RemoteMessage::from_raw(
        id.clone(),
        thread.clone(),
        raw,
        ["SENT".into()].into(),
        mailbox.history_id,
    )?;
    if let Some(thread) = request.thread_id {
        let parents: Vec<_> = mailbox
            .messages
            .values()
            .filter(|m| m.thread_id == thread)
            .collect();
        if parents.is_empty() {
            return Err(Reply::error(404, "notFound"));
        }
        if !parents.iter().any(|parent| {
            !parent.message_id.is_empty()
                && message.in_reply_to == parent.message_id
                && message.references.contains(&parent.message_id)
                && normalized_subject(&message.subject) == normalized_subject(&parent.subject)
        }) {
            return Err(Reply::error(400, "invalidThread"));
        }
    }
    message.history_id = mailbox.next_history();
    let reference = message.reference();
    mailbox.history.push(json!({"id":mailbox.history_id.to_string(),"messages":[reference],"messagesAdded":[{"message":reference}]}));
    mailbox.sends.push(AcceptedSend {
        id: id.clone(),
        thread_id: thread,
        raw: message.raw.clone(),
        message_id: message.message_id.clone(),
    });
    let response = message.wire("minimal", &[])?;
    mailbox.messages.insert(id, message);
    Ok(Reply::json(response))
}
fn normalized_subject(subject: &str) -> String {
    let subject = subject.trim();
    if subject
        .get(..3)
        .is_some_and(|s| s.eq_ignore_ascii_case("re:"))
    {
        normalized_subject(&subject[3..])
    } else {
        subject.to_lowercase()
    }
}
