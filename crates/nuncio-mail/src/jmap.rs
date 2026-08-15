//! JMAP (RFC 8620 / RFC 8621) protocol engine, session discovery, and differential update parser.

use async_trait::async_trait;
use nuncio_core::model::{Email, Folder, Placement, PlacementKey, RemoteIdentity};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::backend::{
    FolderChanges, MailBackend, MutationOutcome, PlacedMessage, RemoteMutationKind,
    RemoteMutationSpec,
};
use crate::parser::MailError;

/// Stand-in for the UIDVALIDITY scope on JMAP, which has no such concept: a
/// JMAP Email object id is already stable within its account (RFC 8621), so a
/// fixed sentinel keeps a placement key the same shape on both protocols
/// without implying a UIDVALIDITY guard. The IMAP UIDVALIDITY guard treats this
/// non-numeric value as "no validity to compare" and is never engaged for a
/// JMAP mutation, which addresses messages by object id.
const JMAP_UID_VALIDITY_SENTINEL: &str = "jmap";

/// Scheme tag prefixing every folder checkpoint this engine writes:
/// `"v1:jmapstate:{state}"`.
///
/// A JMAP state string is an opaque server token (RFC 8620 section 1.6.2) with
/// no self-describing shape, so a stored checkpoint that carries no scheme
/// cannot be told apart from one written by a different protocol engine or a
/// different meaning of the same field. Tagging makes an unrecognised token
/// resolve to a full re-enumeration instead of being handed to `Email/changes`
/// as a `sinceState` it never came from -- over-fetching is recoverable,
/// resuming from a token the server reads differently is not.
const JMAP_STATE_SCHEME: &str = "v1:jmapstate";

/// Separator between the scheme tag and the opaque state it wraps. The state
/// itself may contain further separators, so only the first one is structural.
const JMAP_STATE_DELIM: char = ':';

/// Ceiling on `Email/changes` pages followed in one pass.
///
/// `hasMoreChanges` is a server-driven loop, and a server that answers `true`
/// forever -- or returns a `newState` that never advances -- would otherwise
/// pin the sync task in place. The bound is far above any real paging depth, so
/// hitting it means the server is misbehaving and the pass says so rather than
/// spinning.
const MAX_CHANGES_PAGES: usize = 256;

/// Wrap an opaque JMAP state token in this build's checkpoint scheme.
fn encode_jmap_checkpoint(state: &str) -> String {
    format!("{JMAP_STATE_SCHEME}{JMAP_STATE_DELIM}{state}")
}

/// Recover the opaque JMAP state from a stored checkpoint, or `None` when the
/// token carries no scheme this build recognises. Only the scheme's own
/// delimiter is consumed; the remainder is the server's token verbatim.
fn parse_jmap_checkpoint(raw: &str) -> Option<&str> {
    let state = raw
        .trim()
        .strip_prefix(JMAP_STATE_SCHEME)?
        .strip_prefix(JMAP_STATE_DELIM)?;
    (!state.is_empty()).then_some(state)
}

/// JMAP Session Object (RFC 8620 Section 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmapSession {
    /// Username or account identifier.
    #[serde(default)]
    pub username: String,
    /// Primary accounts map.
    #[serde(default)]
    pub primary_accounts: std::collections::HashMap<String, String>,
    /// JMAP API endpoint URL.
    pub api_url: String,
    /// Download URL for raw message RFC822 blobs.
    #[serde(default)]
    pub download_url: String,
    /// Upload URL for draft attachments.
    #[serde(default)]
    pub upload_url: String,
    /// Event source URL for Server-Sent Events (SSE) push updates.
    #[serde(default)]
    pub event_source_url: String,
    /// Current session state token.
    pub state: String,
}

/// JMAP `Email/get` response payload wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmapEmailGetResponse {
    /// JMAP state token.
    pub state: String,
    /// List of retrieved email objects.
    pub list: Vec<JmapEmail>,
}

/// One page of a JMAP `Email/changes` response (RFC 8621 section 4.2, over the
/// RFC 8620 section 5.2 core method).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmapEmailChangesResponse {
    /// Old state token queried against.
    #[serde(default)]
    pub old_state: String,
    /// New state token after this page of changes.
    pub new_state: String,
    /// Ids of objects created since `oldState`.
    #[serde(default)]
    pub created: Vec<String>,
    /// Ids of objects updated since `oldState`.
    #[serde(default)]
    pub updated: Vec<String>,
    /// Ids of objects destroyed since `oldState`.
    #[serde(default)]
    pub destroyed: Vec<String>,
    /// Whether the server truncated this page. When `true`, `newState` is an
    /// intermediate checkpoint and the caller must ask again from it.
    #[serde(default)]
    pub has_more_changes: bool,
}

/// What an `Email/changes` call established.
///
/// The `cannotCalculateChanges` arm is a first-class outcome rather than an
/// error: RFC 8620 section 5.2 defines it as the server declining to compute a
/// delta from the given state (the state is too old, or the server never kept
/// enough history), and the prescribed response is to re-enumerate rather than
/// to fail the sync.
#[derive(Debug, Clone)]
pub enum EmailChangesPage {
    /// The server answered with a page of changes.
    Changes(JmapEmailChangesResponse),
    /// The server cannot compute a delta from the supplied `sinceState`.
    CannotCalculateChanges,
}

/// JMAP `Email/query` response payload wrapper (RFC 8620 Section 5.5), narrowed
/// to just the matched id list -- `Email/get` needs explicit ids (or `None`
/// for "all", which real servers reject for large mailboxes), so a query must
/// always precede a get.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmapEmailQueryResponse {
    /// Matched email ids, in server-defined order.
    #[serde(default)]
    pub ids: Vec<String>,
}

/// Raw JMAP email object representation (RFC 8621 Section 4.1). Field names
/// must stay `camelCase` on the wire -- `receivedAt`/`isUnread`/`bodySnippet`
/// are the actual JMAP property names; without `rename_all` these silently
/// fail to match and every parsed message gets a zero timestamp, `read:
/// true`, and no snippet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmapEmail {
    pub id: String,
    pub subject: Option<String>,
    pub from: Option<Vec<JmapAddress>>,
    pub to: Option<Vec<JmapAddress>>,
    pub received_at: Option<i64>,
    pub is_unread: Option<bool>,
    pub body_snippet: Option<String>,
    /// RFC 8621 models `messageId` as a list, because a message may carry more
    /// than one `Message-ID` header. The first entry is the one used.
    #[serde(default)]
    pub message_id: Option<Vec<String>>,
    /// Mailboxes this message currently belongs to, as `{mailboxId: true}`
    /// (RFC 8621 section 4.1.1). JMAP models mailbox membership as a set, so
    /// this is the only thing that says which folders a message is placed in --
    /// and, for a message the server reports as changed, whether it is still
    /// placed in the folder being synced at all.
    ///
    /// `None` means the property was not requested or not returned, which is
    /// "unknown membership", never "belongs to nothing".
    #[serde(default)]
    pub mailbox_ids: Option<std::collections::HashMap<String, bool>>,
}

/// JMAP email address object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JmapAddress {
    pub email: String,
}

/// Raw JMAP `Mailbox` object representation (RFC 8621 Section 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmapMailbox {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub total_emails: u32,
    #[serde(default)]
    pub unread_emails: u32,
}

/// JMAP `Mailbox/get` response payload wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmapMailboxGetResponse {
    /// List of retrieved mailbox objects.
    pub list: Vec<JmapMailbox>,
}

/// JMAP protocol engine implementing RFC 8620 / 8621. Mirrors [`crate::imap::ImapEngine`]'s
/// shape: `new` builds an uncredentialed engine that keeps the deterministic, network-free
/// fallback behaviour existing preview/dry-run callers and unit tests rely on; `with_credentials`
/// is what production (`nunciod::sync::build_mail_backend`) always uses, and performs genuine
/// HTTP session discovery and JSON-RPC calls against the real server.
pub struct JmapEngine {
    account_id: String,
    host: String,
    username: Option<String>,
    password: Option<String>,
    http: reqwest::Client,
}

impl JmapEngine {
    /// Create a new, uncredentialed `JmapEngine` bound to an account ID. Never performs
    /// network I/O -- callers that need a real server connection must use
    /// [`JmapEngine::with_credentials`].
    pub fn new(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            host: String::new(),
            username: None,
            password: None,
            http: reqwest::Client::new(),
        }
    }

    /// Create a new `JmapEngine` with a server host and Basic-auth credentials. This is the
    /// production path: `sync_folders`/`sync_messages` perform real HTTP session discovery and
    /// JMAP JSON-RPC calls against `host`.
    pub fn with_credentials(account_id: &str, host: &str, username: &str, password: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            host: host.to_string(),
            username: Some(username.to_string()),
            password: Some(password.to_string()),
            http: reqwest::Client::new(),
        }
    }

    fn has_credentials(&self) -> bool {
        self.username.is_some() && self.password.is_some()
    }

    /// Construct standard JMAP Well-Known session discovery URL (RFC 8620 Section 2.1).
    pub fn build_session_url(host: &str) -> String {
        if host.starts_with("http://") || host.starts_with("https://") {
            let clean = host.trim_end_matches('/');
            format!("{}/.well-known/jmap", clean)
        } else {
            format!("https://{}/.well-known/jmap", host.trim_end_matches('/'))
        }
    }

    /// Build JSON-RPC request invocation for `Email/get`.
    pub fn build_email_get_request(account_id: &str, ids: Option<Vec<String>>) -> Value {
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                [
                    "Email/get",
                    {
                        "accountId": account_id,
                        "ids": ids,
                        // `mailboxIds` is requested because a changed message's
                        // folder membership is what decides whether this pass
                        // places it or reconciles it away.
                        "properties": ["id", "subject", "from", "to", "receivedAt", "isUnread", "bodySnippet", "messageId", "mailboxIds"]
                    },
                    "c1"
                ]
            ]
        })
    }

    /// Build JSON-RPC request invocation for `Email/query`, filtered to a single mailbox.
    /// `Email/get` requires an explicit id list (or `None` for "all", which real servers
    /// reject for anything but tiny mailboxes), so a query must always precede a get.
    pub fn build_email_query_request(account_id: &str, mailbox_id: &str) -> Value {
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                [
                    "Email/query",
                    {
                        "accountId": account_id,
                        "filter": { "inMailboxes": [mailbox_id] }
                    },
                    "c1"
                ]
            ]
        })
    }

    /// Build a JSON-RPC `Email/set` request applying `spec` to a single
    /// message (RFC 8621 s4.6). A `Move`/`Copy`/`SetFlagged` becomes an
    /// `update` patch keyed by the JMAP object id; a `Delete` becomes a
    /// `destroy`.
    ///
    /// `Move` replaces the message's mailbox set with the destination (removing
    /// the source), whereas `Copy` adds the destination via a `mailboxIds/{id}`
    /// patch, leaving the existing membership intact. Flag toggles patch the
    /// `$flagged` keyword (`true` to set, `null` to clear), the JMAP equivalent
    /// of IMAP `\Flagged`.
    pub fn build_email_set_request(account_id: &str, spec: &RemoteMutationSpec) -> Value {
        // Address the message by its protocol-native JMAP object id, never the
        // opaque surrogate `message_id`.
        let id = &spec.remote_id;
        let (update, destroy) = match &spec.kind {
            RemoteMutationKind::SetFlagged { value } => {
                let keyword = if *value { json!(true) } else { Value::Null };
                (json!({ id: { "keywords/$flagged": keyword } }), json!([]))
            }
            RemoteMutationKind::Move { to_folder } => (
                json!({ id: { "mailboxIds": { to_folder: true } } }),
                json!([]),
            ),
            RemoteMutationKind::Copy { to_folder } => (
                json!({ id: { format!("mailboxIds/{to_folder}"): true } }),
                json!([]),
            ),
            RemoteMutationKind::Delete => (json!({}), json!([id])),
        };

        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                [
                    "Email/set",
                    {
                        "accountId": account_id,
                        "update": update,
                        "destroy": destroy
                    },
                    "c1"
                ]
            ]
        })
    }

    /// Confirm a JMAP `Email/set` response genuinely applied the mutation to
    /// `message_id`: the id must appear in `updated`/`destroyed` and must NOT
    /// appear in `notUpdated`/`notDestroyed`. Anything else -- a server-side
    /// rejection, or an id the server silently ignored -- surfaces as an honest
    /// error so the outbox never marks a rejected mutation completed.
    pub fn confirm_email_set_applied(
        raw_json: &str,
        message_id: &str,
        expect_destroy: bool,
    ) -> Result<(), MailError> {
        let val: Value = serde_json::from_str(raw_json)
            .map_err(|e| MailError::ParseFailed(format!("invalid JMAP JSON response: {e}")))?;
        let payload = Self::extract_method_response_payload(&val)?;

        // A rejection is reported in notUpdated/notDestroyed keyed by id; treat
        // its presence as the authoritative failure signal.
        let rejected = payload
            .get(if expect_destroy {
                "notDestroyed"
            } else {
                "notUpdated"
            })
            .and_then(|v| v.as_object())
            .is_some_and(|m| m.contains_key(message_id));
        if rejected {
            return Err(MailError::OperationFailed(format!(
                "JMAP Email/set rejected mutation for message '{message_id}'"
            )));
        }

        let applied = if expect_destroy {
            payload
                .get("destroyed")
                .and_then(|v| v.as_array())
                .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(message_id)))
        } else {
            payload
                .get("updated")
                .and_then(|v| v.as_object())
                .is_some_and(|m| m.contains_key(message_id))
        };
        if applied {
            Ok(())
        } else {
            Err(MailError::OperationFailed(format!(
                "JMAP Email/set did not confirm the mutation for message '{message_id}'"
            )))
        }
    }

    /// Build JSON-RPC request invocation for `Email/changes` (differential sync).
    pub fn build_email_changes_request(account_id: &str, since_state: &str) -> Value {
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                [
                    "Email/changes",
                    {
                        "accountId": account_id,
                        "sinceState": since_state
                    },
                    "c1"
                ]
            ]
        })
    }

    /// Build JSON-RPC request invocation for `Mailbox/get` (all mailboxes).
    pub fn build_mailbox_get_request(account_id: &str) -> Value {
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                [
                    "Mailbox/get",
                    {
                        "accountId": account_id,
                        "ids": null
                    },
                    "c1"
                ]
            ]
        })
    }

    /// Parse JMAP RFC 8620 session response JSON.
    pub fn parse_session(raw_json: &str) -> Result<JmapSession, MailError> {
        serde_json::from_str(raw_json)
            .map_err(|e| MailError::ParseFailed(format!("invalid JMAP session JSON: {e}")))
    }

    /// Extract the argument payload of the first method call from a JMAP JSON-RPC response,
    /// supporting both the full `methodResponses` envelope and a bare response object (used by
    /// tests that exercise a parser directly against a single method's payload).
    fn extract_method_response_payload(val: &Value) -> Result<Value, MailError> {
        if let Some(calls) = val.get("methodResponses").and_then(|v| v.as_array()) {
            let first_call = calls
                .first()
                .ok_or_else(|| MailError::ParseFailed("empty methodResponses array".to_string()))?;
            let args = first_call.get(1).ok_or_else(|| {
                MailError::ParseFailed("missing method response payload".to_string())
            })?;
            Ok(args.clone())
        } else {
            Ok(val.clone())
        }
    }

    /// Parse a raw JMAP `Email/get` JSON response payload into the occupancies
    /// it establishes for `folder_id`, plus the account's Email state string.
    ///
    /// The caller supplies the mailbox being parsed for because the placements
    /// this produces are keyed by it: a placement labelled with a folder the
    /// account does not sync can never be matched again, so a fixed label would
    /// silently strand every message it produced.
    ///
    /// A returned message whose `mailboxIds` is present and does NOT name
    /// `folder_id` is dropped rather than placed: `Email/changes` is
    /// account-wide (RFC 8621 section 4.2 offers no mailbox filter), so a
    /// folder pass sees ids that belong to other mailboxes, and placing them
    /// here would invent an occupancy the server never reported. Membership
    /// that the server did not return at all is left alone -- unknown is not
    /// absent.
    pub fn parse_email_get_response(
        &self,
        raw_json: &str,
        folder_id: &str,
    ) -> Result<(Vec<PlacedMessage>, String), MailError> {
        let val: Value = serde_json::from_str(raw_json)
            .map_err(|e| MailError::ParseFailed(format!("invalid JMAP JSON response: {e}")))?;
        let payload = Self::extract_method_response_payload(&val)?;
        let resp: JmapEmailGetResponse = serde_json::from_value(payload)
            .map_err(|e| MailError::ParseFailed(format!("invalid Email/get payload: {e}")))?;

        let emails = resp
            .list
            .into_iter()
            .filter(|item| match &item.mailbox_ids {
                Some(ids) => ids.get(folder_id).copied().unwrap_or(false),
                None => true,
            })
            .map(|item| {
                let sender = item
                    .from
                    .as_ref()
                    .and_then(|f| f.first())
                    .map(|a| a.email.clone())
                    .unwrap_or_else(|| "unknown@nuncio.mx".to_string());

                let recipient = item
                    .to
                    .as_ref()
                    .and_then(|t| t.first())
                    .map(|a| a.email.clone())
                    .unwrap_or_else(|| "me@nuncio.mx".to_string());

                let folder_id = folder_id.to_string();
                let remote_id = item.id;
                let message_id = item
                    .message_id
                    .as_ref()
                    .and_then(|ids| ids.first())
                    .and_then(|raw| Email::normalize_message_id(raw));

                // A JMAP Email id is server-assigned and stable across mailboxes
                // within the account, which is the same guarantee RFC 8474
                // EMAILID gives, so it enters at the top tier rather than
                // through the surrogate.
                let identity = Email::derive_message_key(
                    &self.account_id,
                    RemoteIdentity {
                        email_id: Some(&remote_id),
                        gm_msgid: None,
                        message_id: message_id.as_deref(),
                        // `Email/get` returns a `bodySnippet`, not the
                        // message's octets, so there is nothing to hash here
                        // without an extra download per message.
                        content_hash: None,
                    },
                    &folder_id,
                    JMAP_UID_VALIDITY_SENTINEL,
                    &remote_id,
                );
                let placement = Placement {
                    account_id: self.account_id.clone(),
                    folder_id,
                    // JMAP has no UIDVALIDITY; the sentinel keeps the placement
                    // key the same shape as IMAP's without implying a guard.
                    uid_validity: JMAP_UID_VALIDITY_SENTINEL.to_string(),
                    remote_id: remote_id.clone(),
                    read: !item.is_unread.unwrap_or(false),
                };

                PlacedMessage {
                    email: Email {
                        id: identity.key,
                        account_id: self.account_id.clone(),
                        subject: item.subject.unwrap_or_else(|| "No Subject".to_string()),
                        sender,
                        recipient,
                        received_at: item.received_at.unwrap_or(0),
                        body_plain: item.body_snippet,
                        body_html: None,
                        attachments: Vec::new(),
                        message_id,
                        content_hash: None,
                    },
                    source: identity.source,
                    placement,
                }
            })
            .collect();

        Ok((emails, resp.state))
    }

    /// Parse one raw JMAP `Email/changes` JSON response payload.
    ///
    /// A method-level error carries a `type` (RFC 8620 section 3.6.2), which a
    /// successful `Email/changes` payload never does, so the discriminator
    /// works whether the caller hands over the full `methodResponses` envelope
    /// or a bare payload. `cannotCalculateChanges` is returned as an outcome so
    /// the caller can fall back to a full enumeration; any other error type is
    /// a genuine failure and surfaces as one.
    pub fn parse_email_changes_response(raw_json: &str) -> Result<EmailChangesPage, MailError> {
        let val: Value = serde_json::from_str(raw_json)
            .map_err(|e| MailError::ParseFailed(format!("invalid JMAP JSON response: {e}")))?;
        let payload = Self::extract_method_response_payload(&val)?;

        if let Some(error_type) = payload.get("type").and_then(|v| v.as_str()) {
            if error_type == "cannotCalculateChanges" {
                return Ok(EmailChangesPage::CannotCalculateChanges);
            }
            return Err(MailError::OperationFailed(format!(
                "JMAP Email/changes failed with error type '{error_type}'"
            )));
        }

        let resp: JmapEmailChangesResponse = serde_json::from_value(payload)
            .map_err(|e| MailError::ParseFailed(format!("invalid Email/changes payload: {e}")))?;
        Ok(EmailChangesPage::Changes(resp))
    }

    /// Parse raw JMAP `Email/query` JSON response payload into a matched id list.
    pub fn parse_email_query_response(raw_json: &str) -> Result<Vec<String>, MailError> {
        let val: Value = serde_json::from_str(raw_json)
            .map_err(|e| MailError::ParseFailed(format!("invalid JMAP JSON response: {e}")))?;
        let payload = Self::extract_method_response_payload(&val)?;
        let resp: JmapEmailQueryResponse = serde_json::from_value(payload)
            .map_err(|e| MailError::ParseFailed(format!("invalid Email/query payload: {e}")))?;

        Ok(resp.ids)
    }

    /// Parse raw JMAP `Mailbox/get` JSON response payload into a mailbox list.
    pub fn parse_mailbox_get_response(raw_json: &str) -> Result<Vec<JmapMailbox>, MailError> {
        let val: Value = serde_json::from_str(raw_json)
            .map_err(|e| MailError::ParseFailed(format!("invalid JMAP JSON response: {e}")))?;
        let payload = Self::extract_method_response_payload(&val)?;
        let resp: JmapMailboxGetResponse = serde_json::from_value(payload)
            .map_err(|e| MailError::ParseFailed(format!("invalid Mailbox/get payload: {e}")))?;

        Ok(resp.list)
    }

    /// Issue an authenticated JMAP session discovery `GET /.well-known/jmap` against `self.host`.
    /// A network failure or unparseable response surfaces as a genuine [`MailError`] -- there is
    /// no fallback to canned data once credentials are present.
    async fn discover_session(&self) -> Result<JmapSession, MailError> {
        let url = Self::build_session_url(&self.host);
        let mut request = self.http.get(&url);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            request = request.basic_auth(u, Some(p));
        }

        let response = request.send().await.map_err(|e| {
            MailError::NetworkError(format!("JMAP session discovery at {url} failed: {e}"))
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|e| {
            MailError::NetworkError(format!("failed to read JMAP session response body: {e}"))
        })?;
        if !status.is_success() {
            return Err(MailError::NetworkError(format!(
                "JMAP session discovery at {url} returned status {status}"
            )));
        }

        Self::parse_session(&body)
    }

    /// POST a JMAP JSON-RPC request body to `api_url`, returning the raw response text. A
    /// non-2xx status or transport failure surfaces as [`MailError::NetworkError`].
    async fn post_jmap(&self, api_url: &str, body: &Value) -> Result<String, MailError> {
        let mut request = self.http.post(api_url).json(body);
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            request = request.basic_auth(u, Some(p));
        }

        let response = request.send().await.map_err(|e| {
            MailError::NetworkError(format!("JMAP request to {api_url} failed: {e}"))
        })?;
        let status = response.status();
        let text = response.text().await.map_err(|e| {
            MailError::NetworkError(format!("failed to read JMAP response body: {e}"))
        })?;
        if !status.is_success() {
            return Err(MailError::NetworkError(format!(
                "JMAP server at {api_url} returned status {status}"
            )));
        }

        Ok(text)
    }

    /// Resolve the JMAP `accountId` to use for API calls: the session's advertised primary mail
    /// account if present, else the Nuncio account id as a last resort.
    fn resolve_account_id(&self, session: &JmapSession) -> String {
        session
            .primary_accounts
            .get("urn:ietf:params:jmap:mail")
            .cloned()
            .unwrap_or_else(|| self.account_id.clone())
    }

    /// Address one occupancy of `remote_id` in `folder_id`. JMAP has no
    /// UIDVALIDITY, so the sentinel stands in for that half of the key.
    fn placement_key(&self, folder_id: &str, remote_id: &str) -> PlacementKey {
        PlacementKey {
            account_id: self.account_id.clone(),
            folder_id: folder_id.to_string(),
            uid_validity: JMAP_UID_VALIDITY_SENTINEL.to_string(),
            remote_id: remote_id.to_string(),
        }
    }

    /// Follow `Email/changes` from `since_state` until the server stops setting
    /// `hasMoreChanges`, accumulating every page.
    ///
    /// Returns `None` when the server answered `cannotCalculateChanges`, which
    /// is the caller's cue to re-enumerate instead of failing. Stopping at the
    /// first page would silently drop every change past the server's page
    /// limit, so the loop is the contract, not an optimisation.
    async fn collect_email_changes(
        &self,
        api_url: &str,
        jmap_account_id: &str,
        since_state: &str,
    ) -> Result<Option<EmailChangeSet>, MailError> {
        let mut accumulated = EmailChangeSet {
            changed: Vec::new(),
            destroyed: Vec::new(),
            new_state: since_state.to_string(),
        };
        let mut seen_changed: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_destroyed: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut cursor = since_state.to_string();

        for _ in 0..MAX_CHANGES_PAGES {
            let request = Self::build_email_changes_request(jmap_account_id, &cursor);
            let raw = self.post_jmap(api_url, &request).await?;
            let page = match Self::parse_email_changes_response(&raw)? {
                EmailChangesPage::CannotCalculateChanges => return Ok(None),
                EmailChangesPage::Changes(page) => page,
            };

            for id in page.created.into_iter().chain(page.updated) {
                if seen_changed.insert(id.clone()) {
                    accumulated.changed.push(id);
                }
            }
            for id in page.destroyed {
                if seen_destroyed.insert(id.clone()) {
                    accumulated.destroyed.push(id);
                }
            }
            accumulated.new_state = page.new_state.clone();

            if !page.has_more_changes {
                // A message created and then destroyed within the same window
                // appears in both lists; it is gone, so absence wins.
                accumulated
                    .changed
                    .retain(|id| !seen_destroyed.contains(id));
                return Ok(Some(accumulated));
            }
            if page.new_state == cursor {
                return Err(MailError::OperationFailed(
                    "JMAP Email/changes reported more changes without advancing newState"
                        .to_string(),
                ));
            }
            cursor = page.new_state;
        }

        Err(MailError::OperationFailed(format!(
            "JMAP Email/changes did not converge within {MAX_CHANGES_PAGES} pages"
        )))
    }

    /// Enumerate a folder in full: every message it currently holds, and a
    /// `present` set that says so.
    ///
    /// This is the only pass entitled to report presence, because it is the
    /// only one that genuinely looked at the whole mailbox.
    async fn full_folder_enumeration(
        &self,
        api_url: &str,
        jmap_account_id: &str,
        folder_id: &str,
    ) -> Result<FolderChanges, MailError> {
        let query_request = Self::build_email_query_request(jmap_account_id, folder_id);
        let query_raw = self.post_jmap(api_url, &query_request).await?;
        let ids = Self::parse_email_query_response(&query_raw)?;

        let get_request = Self::build_email_get_request(jmap_account_id, Some(ids));
        let get_raw = self.post_jmap(api_url, &get_request).await?;
        let (emails, state) = self.parse_email_get_response(&get_raw, folder_id)?;

        let present: Vec<PlacementKey> = emails.iter().map(|m| m.placement.key()).collect();

        tracing::info!(
            account_id = %self.account_id,
            folder_id,
            fetched = emails.len(),
            "jmap full folder enumeration complete"
        );

        Ok(FolderChanges {
            upserts: emails,
            // Everything absent is expressed through `present`; a full pass has
            // no separate report of removals to make.
            removals: Vec::new(),
            present: Some(present),
            next_state: encode_jmap_checkpoint(&state),
        })
    }

    /// Enumerate one folder's changes since `since_state` via `Email/changes`.
    ///
    /// Returns `None` when the server cannot compute the delta, so the caller
    /// can fall back to a full enumeration.
    async fn incremental_folder_pass(
        &self,
        api_url: &str,
        jmap_account_id: &str,
        folder_id: &str,
        since_state: &str,
    ) -> Result<Option<FolderChanges>, MailError> {
        let Some(changes) = self
            .collect_email_changes(api_url, jmap_account_id, since_state)
            .await?
        else {
            return Ok(None);
        };

        let upserts = if changes.changed.is_empty() {
            Vec::new()
        } else {
            let get_request =
                Self::build_email_get_request(jmap_account_id, Some(changes.changed.clone()));
            let get_raw = self.post_jmap(api_url, &get_request).await?;
            let (emails, _state) = self.parse_email_get_response(&get_raw, folder_id)?;
            emails
        };

        // Destruction is account-wide, so a destroyed id takes its occupancy
        // here with it. A changed id that came back placed somewhere other than
        // this folder -- or did not come back at all, having been destroyed
        // between the two calls -- has left this folder, and its occupancy goes
        // too. The message itself survives as long as another placement does;
        // that is the caller's decision, not this one's.
        let placed: std::collections::HashSet<&str> = upserts
            .iter()
            .map(|m| m.placement.remote_id.as_str())
            .collect();
        let removals: Vec<PlacementKey> = changes
            .destroyed
            .iter()
            .map(String::as_str)
            .chain(
                changes
                    .changed
                    .iter()
                    .map(String::as_str)
                    .filter(|id| !placed.contains(id)),
            )
            .map(|id| self.placement_key(folder_id, id))
            .collect();

        tracing::info!(
            account_id = %self.account_id,
            folder_id,
            fetched = upserts.len(),
            removed = removals.len(),
            "jmap incremental folder sync complete"
        );

        Ok(Some(FolderChanges {
            upserts,
            removals,
            // An `Email/changes` pass sees only what moved. It never enumerates
            // the mailbox, so it cannot speak to absence beyond what the server
            // explicitly reported, and claiming otherwise would delete every
            // message that simply did not change.
            present: None,
            next_state: encode_jmap_checkpoint(&changes.new_state),
        }))
    }
}

/// Every `Email/changes` page for one pass, collapsed.
struct EmailChangeSet {
    /// Created and updated ids, deduplicated, in first-seen order.
    changed: Vec<String>,
    /// Destroyed ids, deduplicated, in first-seen order.
    destroyed: Vec<String>,
    /// The final `newState`, once the server stopped paging.
    new_state: String,
}

#[async_trait]
impl MailBackend for JmapEngine {
    #[tracing::instrument(skip(self), fields(account_id = %self.account_id), err)]
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError> {
        if !self.has_credentials() {
            return Err(MailError::AuthError(
                "JMAP folder sync requires credentials".to_string(),
            ));
        }
        let session = self.discover_session().await?;
        let account_id = self.resolve_account_id(&session);
        let request = Self::build_mailbox_get_request(&account_id);
        let raw = self.post_jmap(&session.api_url, &request).await?;
        let mailboxes = Self::parse_mailbox_get_response(&raw)?;

        let folders: Vec<Folder> = mailboxes
            .into_iter()
            .map(|mb| Folder {
                id: mb.id,
                name: mb.name,
                total_messages: mb.total_emails as usize,
                unread_messages: mb.unread_emails as usize,
            })
            .collect();

        tracing::info!(
            account_id = %self.account_id,
            folders = folders.len(),
            "jmap folder list sync complete"
        );

        Ok(folders)
    }

    #[tracing::instrument(
        skip(self, since_state),
        fields(account_id = %self.account_id, folder_id = %folder_id),
        err
    )]
    async fn sync_changes(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<FolderChanges, MailError> {
        if !self.has_credentials() {
            return Err(MailError::AuthError(
                "JMAP message sync requires credentials".to_string(),
            ));
        }
        let session = self.discover_session().await?;
        let account_id = self.resolve_account_id(&session);

        // A checkpoint this build wrote resumes differentially. Anything else
        // -- absent, or carrying no scheme this build recognises -- enumerates
        // the folder in full, which is always safe.
        if let Some(state) = since_state.and_then(parse_jmap_checkpoint) {
            if let Some(changes) = self
                .incremental_folder_pass(&session.api_url, &account_id, folder_id, state)
                .await?
            {
                return Ok(changes);
            }
            tracing::info!(
                account_id = %self.account_id,
                folder_id,
                "jmap server cannot calculate changes from the stored state; re-enumerating"
            );
        }

        self.full_folder_enumeration(&session.api_url, &account_id, folder_id)
            .await
    }

    #[tracing::instrument(skip(self), fields(account_id = %self.account_id), err)]
    async fn apply_mutation(
        &self,
        spec: &RemoteMutationSpec,
    ) -> Result<MutationOutcome, MailError> {
        if !self.has_credentials() {
            return Err(MailError::AuthError(
                "JMAP remote mutation requires credentials".to_string(),
            ));
        }

        // JMAP has no UIDVALIDITY-equivalent guard (an `Email/set` targets a
        // stable object id per RFC 8621), but a destructive mutation is still
        // logged BEFORE the request is issued, mirroring the IMAP audit trail.
        match &spec.kind {
            RemoteMutationKind::Move { to_folder } => {
                tracing::info!(
                    op = "MOVE",
                    folder_id = %spec.folder_id,
                    to_folder = %to_folder,
                    remote_id = %spec.remote_id,
                    "issuing destructive JMAP mailbox move (Email/set update)"
                );
            }
            RemoteMutationKind::Delete => {
                tracing::warn!(
                    op = "DELETE",
                    folder_id = %spec.folder_id,
                    remote_id = %spec.remote_id,
                    "issuing destructive JMAP delete (Email/set destroy)"
                );
            }
            RemoteMutationKind::SetFlagged { .. } | RemoteMutationKind::Copy { .. } => {}
        }

        let session = self.discover_session().await?;
        let account_id = self.resolve_account_id(&session);

        let request = Self::build_email_set_request(&account_id, spec);
        let raw = self.post_jmap(&session.api_url, &request).await?;
        let expect_destroy = matches!(spec.kind, RemoteMutationKind::Delete);
        // `Email/set` names the ids it updated or destroyed, so a confirmed
        // response is genuine proof rather than a bare acknowledgement. JMAP
        // has no COPYUID-style ambiguity here; the id is stable and the server
        // either lists it or does not.
        Self::confirm_email_set_applied(&raw, &spec.remote_id, expect_destroy)?;
        Ok(MutationOutcome::Applied { token: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_session_url_formatting() {
        let url = JmapEngine::build_session_url("mail.kof22.com");
        assert_eq!(url, "https://mail.kof22.com/.well-known/jmap");
    }

    #[test]
    fn build_email_get_request_json_structure() {
        let req = JmapEngine::build_email_get_request("acct-1", Some(vec!["msg-1".to_string()]));
        assert!(req["methodCalls"].is_array());
        assert_eq!(req["methodCalls"][0][0], "Email/get");
    }

    #[test]
    fn build_email_changes_request_json_structure() {
        let req = JmapEngine::build_email_changes_request("acct-1", "state-1");
        assert_eq!(req["methodCalls"][0][0], "Email/changes");
        assert_eq!(req["methodCalls"][0][1]["sinceState"], "state-1");
    }

    #[test]
    fn build_email_query_request_json_structure() {
        let req = JmapEngine::build_email_query_request("acct-1", "mailbox-1");
        assert_eq!(req["methodCalls"][0][0], "Email/query");
        assert_eq!(
            req["methodCalls"][0][1]["filter"]["inMailboxes"][0],
            "mailbox-1"
        );
    }

    #[test]
    fn build_mailbox_get_request_json_structure() {
        let req = JmapEngine::build_mailbox_get_request("acct-1");
        assert_eq!(req["methodCalls"][0][0], "Mailbox/get");
        assert_eq!(req["methodCalls"][0][1]["accountId"], "acct-1");
    }

    #[test]
    fn parse_session_valid_payload() {
        let raw = r#"{
            "username": "james",
            "apiUrl": "https://mail.kof22.com/jmap/api",
            "state": "sess-42"
        }"#;
        let session = JmapEngine::parse_session(raw).expect("parse session");
        assert_eq!(session.username, "james");
        assert_eq!(session.api_url, "https://mail.kof22.com/jmap/api");
        assert_eq!(session.state, "sess-42");
    }

    #[test]
    fn parse_email_changes_response_valid_payload() {
        let raw = r#"{
            "oldState": "s-1",
            "newState": "s-2",
            "created": ["msg-12"],
            "updated": ["msg-10", "msg-11"],
            "destroyed": ["msg-5"],
            "hasMoreChanges": true
        }"#;
        let EmailChangesPage::Changes(page) =
            JmapEngine::parse_email_changes_response(raw).expect("parse changes")
        else {
            panic!("a well-formed page must not read as cannotCalculateChanges");
        };
        assert_eq!(page.created, vec!["msg-12".to_string()]);
        assert_eq!(page.updated.len(), 2);
        assert_eq!(page.destroyed.len(), 1);
        assert_eq!(page.new_state, "s-2");
        assert!(page.has_more_changes);
    }

    /// `cannotCalculateChanges` is an outcome, not a failure: RFC 8620 s5.2
    /// defines it as "re-enumerate", so it must not surface as an error that
    /// aborts the sync. Any other method error still must.
    #[test]
    fn parse_email_changes_response_distinguishes_cannot_calculate_from_failure() {
        let raw = r#"{"methodResponses":[["error",{"type":"cannotCalculateChanges"},"c1"]]}"#;
        assert!(matches!(
            JmapEngine::parse_email_changes_response(raw),
            Ok(EmailChangesPage::CannotCalculateChanges)
        ));

        let failed = r#"{"methodResponses":[["error",{"type":"serverFail"},"c1"]]}"#;
        assert!(JmapEngine::parse_email_changes_response(failed).is_err());
    }

    #[test]
    fn jmap_checkpoints_round_trip_through_their_scheme() {
        // A JMAP state is opaque and may itself contain the delimiter, so only
        // the scheme's own separator is structural.
        let encoded = encode_jmap_checkpoint("state:with:colons");
        assert_eq!(encoded, "v1:jmapstate:state:with:colons");
        assert_eq!(parse_jmap_checkpoint(&encoded), Some("state:with:colons"));
    }

    #[test]
    fn an_untagged_or_unknown_scheme_checkpoint_is_not_interpreted() {
        // A bare token written before tagging existed, or one from another
        // scheme, must resolve to a full re-enumeration rather than being
        // handed to the server as a sinceState it never issued.
        assert_eq!(parse_jmap_checkpoint("sync-state-900"), None);
        assert_eq!(parse_jmap_checkpoint("v1:uidnext:42:105"), None);
        assert_eq!(parse_jmap_checkpoint("v1:jmapstate:"), None);
    }

    #[test]
    fn parse_email_get_response_drops_messages_placed_in_another_mailbox() {
        // `Email/changes` is account-wide, so a folder pass sees ids that
        // belong elsewhere. Membership the server did return is authoritative;
        // membership it never returned is unknown, not absent.
        let raw = r#"{
            "methodResponses": [
                ["Email/get", {"state": "s-1", "list": [
                    {"id": "m-here", "mailboxIds": {"mb-inbox": true}},
                    {"id": "m-elsewhere", "mailboxIds": {"mb-archive": true}},
                    {"id": "m-unknown"}
                ]}, "c1"]
            ]
        }"#;
        let engine = JmapEngine::new("acct-1");
        let (emails, _state) = engine
            .parse_email_get_response(raw, "mb-inbox")
            .expect("parse get");
        let ids: Vec<&str> = emails
            .iter()
            .map(|m| m.placement.remote_id.as_str())
            .collect();
        assert_eq!(ids, vec!["m-here", "m-unknown"]);
        assert_eq!(emails[0].placement.folder_id, "mb-inbox");
    }

    #[test]
    fn parse_email_get_response_captures_normalized_message_id() {
        // RFC 8621 models `messageId` as a list; the first entry is taken and
        // normalized to the same canonical form the IMAP paths produce, so two
        // engines syncing one account via different protocols agree.
        let raw = r#"{
            "methodResponses": [
                ["Email/get", {"state": "s-1", "list": [
                    {"id": "m-1", "subject": "Hi", "messageId": ["<Abc.123@mail.nuncio.mx>"]},
                    {"id": "m-2", "subject": "No id"}
                ]}, "c1"]
            ]
        }"#;
        let engine = JmapEngine::new("acct-1");
        let (emails, _state) = engine
            .parse_email_get_response(raw, "mb-inbox")
            .expect("parse get");

        assert_eq!(
            emails[0].email.message_id.as_deref(),
            Some("Abc.123@mail.nuncio.mx")
        );
        assert_eq!(
            emails[1].email.message_id, None,
            "an absent messageId must stay None, not become empty"
        );
        // `Email/get` returns a snippet, never the octets, so there is nothing
        // honest to hash on this path.
        assert_eq!(emails[0].email.content_hash, None);
    }

    #[test]
    fn parse_invalid_jmap_json_fails() {
        let engine = JmapEngine::new("acct-1");
        let res = engine.parse_email_get_response("{ invalid json }", "mb-inbox");
        assert!(res.is_err());
    }

    #[test]
    fn parse_email_query_response_extracts_ids() {
        let raw = r#"{
            "methodResponses": [
                ["Email/query", {"ids": ["msg-1", "msg-2"]}, "c1"]
            ]
        }"#;
        let ids = JmapEngine::parse_email_query_response(raw).expect("parse query");
        assert_eq!(ids, vec!["msg-1".to_string(), "msg-2".to_string()]);
    }

    #[test]
    fn parse_mailbox_get_response_extracts_mailboxes() {
        let raw = r#"{
            "methodResponses": [
                ["Mailbox/get", {"list": [
                    {"id": "mb-1", "name": "Inbox", "totalEmails": 5, "unreadEmails": 2}
                ]}, "c1"]
            ]
        }"#;
        let mailboxes = JmapEngine::parse_mailbox_get_response(raw).expect("parse mailboxes");
        assert_eq!(mailboxes.len(), 1);
        assert_eq!(mailboxes[0].id, "mb-1");
        assert_eq!(mailboxes[0].total_emails, 5);
        assert_eq!(mailboxes[0].unread_emails, 2);
    }

    /// Regression test for the camelCase wire-format bug: `JmapEmail` must deserialize
    /// `receivedAt`/`isUnread`/`bodySnippet` (the real JMAP wire property names) rather than
    /// silently defaulting them to `None` because it only matched the Rust field names.
    #[test]
    fn jmap_email_deserializes_camel_case_wire_fields() {
        let raw = r#"{
            "id": "msg-1",
            "subject": "Test",
            "from": [{"email": "a@nuncio.mx"}],
            "to": [{"email": "b@nuncio.mx"}],
            "receivedAt": 1700000000,
            "isUnread": true,
            "bodySnippet": "hello world"
        }"#;
        let email: JmapEmail = serde_json::from_str(raw).expect("parse camelCase JmapEmail");
        assert_eq!(email.received_at, Some(1700000000));
        assert_eq!(email.is_unread, Some(true));
        assert_eq!(email.body_snippet, Some("hello world".to_string()));
    }

    fn spec(kind: RemoteMutationKind) -> RemoteMutationSpec {
        RemoteMutationSpec {
            message_id: "surrogate-m-1".to_string(),
            remote_id: "m-1".to_string(),
            folder_id: "mb-inbox".to_string(),
            uid_validity: JMAP_UID_VALIDITY_SENTINEL.to_string(),
            kind,
        }
    }

    #[test]
    fn build_email_set_request_copy_adds_target_without_removing_source() {
        let req = JmapEngine::build_email_set_request(
            "acct-1",
            &spec(RemoteMutationKind::Copy {
                to_folder: "mb-archive".to_string(),
            }),
        );
        let call = &req["methodCalls"][0];
        assert_eq!(call[0], "Email/set");
        // A copy patches a single mailboxIds/{id} pointer to true, leaving the
        // rest of the message's mailbox membership untouched.
        assert_eq!(call[1]["update"]["m-1"]["mailboxIds/mb-archive"], true);
    }

    #[test]
    fn build_email_set_request_unflag_clears_keyword_with_null() {
        let req = JmapEngine::build_email_set_request(
            "acct-1",
            &spec(RemoteMutationKind::SetFlagged { value: false }),
        );
        assert!(req["methodCalls"][0][1]["update"]["m-1"]["keywords/$flagged"].is_null());
    }

    #[test]
    fn confirm_email_set_applied_requires_the_id_in_the_updated_map() {
        let ok = r#"{"methodResponses":[["Email/set",{"updated":{"m-1":null}},"c1"]]}"#;
        assert!(JmapEngine::confirm_email_set_applied(ok, "m-1", false).is_ok());

        // An empty updated map is NOT success -- the server silently ignored it.
        let ignored = r#"{"methodResponses":[["Email/set",{"updated":{}},"c1"]]}"#;
        assert!(JmapEngine::confirm_email_set_applied(ignored, "m-1", false).is_err());
    }

    #[tokio::test]
    async fn uncredentialed_engine_requires_credentials_to_sync() {
        // Without credentials there is no server to discover a session from;
        // both methods must surface an honest auth error rather than
        // fabricating folders or messages.
        let engine = JmapEngine::new("acct-1");
        assert!(!engine.has_credentials());

        let err = engine
            .sync_folders()
            .await
            .expect_err("credential-less sync_folders must fail");
        assert!(matches!(err, MailError::AuthError(_)));

        let err = engine
            .sync_messages("inbox", None)
            .await
            .expect_err("credential-less sync_messages must fail");
        assert!(matches!(err, MailError::AuthError(_)));

        let err = engine
            .apply_mutation(&spec(RemoteMutationKind::Delete))
            .await
            .expect_err("credential-less apply_mutation must fail");
        assert!(matches!(err, MailError::AuthError(_)));
    }
}
