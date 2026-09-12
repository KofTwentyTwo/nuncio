use super::{
    oauth::{Code, Grant},
    wire::{Input, Reply, Result},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

#[derive(Clone, Copy)]
pub enum Seed {
    TwoAccounts,
}

#[derive(Clone)]
pub struct GoogleControl(
    pub(super) Arc<Mutex<Model>>,
    pub(super) tokio::sync::watch::Sender<bool>,
);
impl GoogleControl {
    pub async fn set_calendar_role(
        &self,
        account: &str,
        id: &str,
        role: &str,
    ) -> std::result::Result<(), crate::TestError> {
        if !matches!(
            role,
            "owner" | "writer" | "writerWithoutPrivateAccess" | "reader"
        ) {
            return Err(Reply::error(400, "unsupportedRole").into());
        }
        let mut model = self.0.lock().await;
        let id = if id == "primary" { account } else { id };
        let calendar = model.calendar_mut(account, id)?;
        if calendar.primary && role != "owner" {
            return Err(Reply::error(400, "primaryOwnership").into());
        }
        calendar.access_role = role.into();
        Ok(())
    }
    pub async fn omit_calendar_fields(
        &self,
        account: &str,
        id: &str,
        fields: &[&str],
    ) -> std::result::Result<(), crate::TestError> {
        if fields
            .iter()
            .any(|f| !matches!(*f, "timeZone" | "summary" | "primary" | "accessRole"))
        {
            return Err(Reply::error(400, "invalidArgument").into());
        }
        let id = if id == "primary" { account } else { id };
        self.0
            .lock()
            .await
            .calendar_mut(account, id)?
            .omitted_fields = fields.iter().map(|f| f.to_string()).collect();
        Ok(())
    }
    pub async fn remove_calendar(
        &self,
        account: &str,
        id: &str,
    ) -> std::result::Result<(), crate::TestError> {
        let mut model = self.0.lock().await;
        let id = if id == "primary" { account } else { id };
        model
            .calendars
            .get_mut(account)
            .ok_or_else(|| Reply::error(404, "notFound"))?
            .remove(id)
            .ok_or_else(|| Reply::error(404, "notFound"))?;
        model
            .calendar_tokens
            .retain(|_, c| c.account != account || c.calendar != id);
        Ok(())
    }
    pub async fn omit_message_fields(
        &self,
        account: &str,
        id: &str,
        fields: &[&str],
    ) -> std::result::Result<(), crate::TestError> {
        if fields.iter().any(|f| {
            !matches!(
                *f,
                "threadId"
                    | "historyId"
                    | "internalDate"
                    | "labelIds"
                    | "payload"
                    | "raw"
                    | "sizeEstimate"
            )
        }) {
            return Err(Reply::error(400, "invalidArgument").into());
        }
        self.0
            .lock()
            .await
            .mailbox_mut(account)?
            .messages
            .get_mut(id)
            .ok_or_else(|| Reply::error(404, "notFound"))?
            .omitted_fields = fields.iter().map(|f| f.to_string()).collect();
        Ok(())
    }
    pub async fn add_message(
        &self,
        account: &str,
        id: &str,
        thread: &str,
        raw: Vec<u8>,
        labels: BTreeSet<String>,
    ) -> std::result::Result<(), crate::TestError> {
        let mut model = self.0.lock().await;
        let mailbox = model.mailbox_mut(account)?;
        if mailbox.messages.contains_key(id)
            || labels.iter().any(|id| !mailbox.labels.contains_key(id))
        {
            return Err(Reply::error(400, "invalidArgument").into());
        }
        let mut message = super::mail_state::RemoteMessage::from_raw(
            id.into(),
            thread.into(),
            raw,
            labels,
            mailbox.history_id,
        )?;
        message.history_id = mailbox.next_history();
        let reference = message.reference();
        mailbox.history.push(serde_json::json!({"id":mailbox.history_id.to_string(),"messages":[reference],"messagesAdded":[{"message":reference}]}));
        mailbox.messages.insert(id.into(), message);
        Ok(())
    }
    pub async fn change_labels(
        &self,
        account: &str,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> std::result::Result<(), crate::TestError> {
        self.0
            .lock()
            .await
            .mailbox_mut(account)?
            .change_labels(id, add, remove)?;
        Ok(())
    }
    pub async fn delete_event(
        &self,
        account: &str,
        calendar: &str,
        id: &str,
    ) -> std::result::Result<(), crate::TestError> {
        let mut model = self.0.lock().await;
        let calendar = model.calendar_mut(
            account,
            if calendar == "primary" {
                account
            } else {
                calendar
            },
        )?;
        if !calendar.events.contains_key(id) {
            return Err(Reply::error(404, "notFound").into());
        }
        calendar.put(
            serde_json::json!({"id":id,"status":"cancelled"}),
            "none",
            "external_delete",
        )?;
        Ok(())
    }
    pub async fn reset(&self, seed: Seed) -> std::result::Result<(), crate::TestError> {
        let mut model = self.0.lock().await;
        let mut next = Model::new(seed)?;
        next.sequence = model.sequence + 1;
        for barrier in model.barriers.values() {
            barrier.released.send_replace(true);
        }
        *model = next;
        Ok(())
    }
    pub async fn put_event(
        &self,
        account: &str,
        calendar: &str,
        body: serde_json::Value,
    ) -> std::result::Result<(), crate::TestError> {
        let id = if calendar == "primary" {
            account
        } else {
            calendar
        };
        self.0
            .lock()
            .await
            .calendar_mut(account, id)?
            .put(body, "none", "external")?;
        Ok(())
    }
    pub async fn set_page_overlap(&self, overlap: bool) {
        self.0.lock().await.overlap = overlap;
    }
    pub async fn reverse_results(&self, reverse: bool) {
        self.0.lock().await.reverse = reverse;
    }
    pub async fn set_page_cap(&self, size: usize) -> std::result::Result<(), crate::TestError> {
        if !(1..=2500).contains(&size) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "page cap must be 1..2500",
            )
            .into());
        }
        self.0.lock().await.page_cap = size;
        Ok(())
    }
    pub async fn delete_message(
        &self,
        account: &str,
        id: &str,
    ) -> std::result::Result<(), crate::TestError> {
        let mut model = self.0.lock().await;
        let mailbox = model.mailbox_mut(account)?;
        let message = mailbox
            .messages
            .remove(id)
            .ok_or_else(|| Reply::error(404, "notFound"))?;
        let history = mailbox.next_history();
        let reference = message.reference();
        mailbox.history.push(serde_json::json!({"id":history.to_string(),"messages":[reference],"messagesDeleted":[{"message":reference}]}));
        Ok(())
    }
    pub async fn expire_cursor(
        &self,
        account: &str,
        scope: CursorScope,
    ) -> std::result::Result<(), crate::TestError> {
        let mut model = self.0.lock().await;
        match scope {
            CursorScope::Calendar(calendar) => {
                let calendar = if calendar == "primary" {
                    account
                } else {
                    &calendar
                };
                model
                    .calendar_tokens
                    .retain(|_, cursor| cursor.account != account || cursor.calendar != calendar);
            }
            CursorScope::GmailHistory => {
                let mailbox = model.mailbox_mut(account)?;
                mailbox.history_floor = mailbox.next_history();
                mailbox.history.clear();
            }
            CursorScope::Pages => {
                model
                    .pages
                    .retain(|_, page| !page.scope.starts_with(&format!("{account}|")));
            }
        }
        Ok(())
    }
    pub async fn wait_for_barrier(&self, name: &str) -> std::result::Result<(), crate::TestError> {
        let mut entered = self
            .0
            .lock()
            .await
            .barriers
            .entry(name.into())
            .or_insert_with(super::faults::Barrier::new)
            .entered
            .subscribe();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if *entered.borrow() {
                    return Ok::<_, tokio::sync::watch::error::RecvError>(());
                }
                entered.changed().await?;
            }
        })
        .await??;
        Ok(())
    }
    pub async fn release_barrier(&self, name: &str) {
        self.0
            .lock()
            .await
            .barriers
            .entry(name.into())
            .or_insert_with(super::faults::Barrier::new)
            .released
            .send_replace(true);
    }
    pub async fn inject(&self, fault: super::faults::Fault) {
        self.0.lock().await.faults.push(fault);
    }
    pub async fn snapshot(&self) -> Snapshot {
        let model = self.0.lock().await;
        Snapshot {
            calendars: model
                .calendars
                .iter()
                .map(|(account, calendars)| {
                    (
                        account.clone(),
                        calendars
                            .iter()
                            .map(|(id, c)| (id.clone(), c.snapshot()))
                            .collect(),
                    )
                })
                .collect(),
            requests: model
                .counts
                .iter()
                .map(
                    |((account, method, path), count)| super::faults::RequestCount {
                        account: account.clone(),
                        method: method.clone(),
                        path: path.clone(),
                        count: *count,
                    },
                )
                .collect(),
            mail: model
                .mailboxes
                .iter()
                .map(|(id, mailbox)| (id.clone(), mailbox.snapshot()))
                .collect(),
        }
    }
    pub async fn accepted_sends(&self, account: &str) -> Vec<super::mail_state::AcceptedSend> {
        self.0
            .lock()
            .await
            .mailboxes
            .get(account)
            .map(|m| m.sends.clone())
            .unwrap_or_default()
    }
    pub async fn advance(&self, duration: Duration) {
        self.0.lock().await.now += duration.as_secs();
    }
    pub async fn revoke(&self, account: &str) {
        let mut model = self.0.lock().await;
        model.access.retain(|_, grant| grant.account != account);
        model.refresh.retain(|_, grant| grant.account != account);
    }
    pub async fn deny_consent(&self, deny: bool) {
        self.0.lock().await.deny_consent = deny;
    }
    pub async fn deny_scope(&self, scope: &str) {
        self.0.lock().await.denied_scopes.insert(scope.into());
    }
    pub async fn rotate_refresh_tokens(&self, enabled: bool) {
        self.0.lock().await.rotate_refresh = enabled;
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorScope {
    Calendar(String),
    GmailHistory,
    Pages,
}

#[derive(Clone, serde::Serialize)]
pub struct Snapshot {
    pub calendars: BTreeMap<String, BTreeMap<String, super::calendar_state::CalendarSnapshot>>,
    pub requests: Vec<super::faults::RequestCount>,
    pub mail: BTreeMap<String, super::mail_state::MailboxSnapshot>,
}

pub(super) struct Model {
    issued_credentials: BTreeMap<[u8; 32], String>,
    pub rotate_refresh: bool,
    pub overlap: bool,
    pub calendars: BTreeMap<String, BTreeMap<String, super::calendar_state::Calendar>>,
    pub calendar_tokens: BTreeMap<String, super::calendar_state::SyncCursor>,
    pub counts: super::faults::Counts,
    pub barriers: BTreeMap<String, super::faults::Barrier>,
    pub faults: Vec<super::faults::Fault>,
    pub mailboxes: BTreeMap<String, super::mail_state::Mailbox>,
    pub pages: BTreeMap<String, super::paging::Page>,
    pub page_cap: usize,
    pub reverse: bool,
    pub accounts: BTreeSet<String>,
    pub sequence: u64,
    pub now: u64,
    pub codes: BTreeMap<String, Code>,
    pub access: BTreeMap<String, Grant>,
    pub refresh: BTreeMap<String, Grant>,
    pub deny_consent: bool,
    pub denied_scopes: BTreeSet<String>,
}
impl Model {
    pub fn new(_seed: Seed) -> Result<Self> {
        Ok(Self {
            issued_credentials: BTreeMap::new(),
            rotate_refresh: false,
            overlap: false,
            calendars: ["alpha@example.test", "beta@example.test"]
                .into_iter()
                .map(|account| {
                    let primary = super::calendar_state::Calendar::seeded(account, true)?;
                    let team = super::calendar_state::Calendar::seeded(account, false)?;
                    Ok((
                        account.into(),
                        [(primary.id.clone(), primary), (team.id.clone(), team)].into(),
                    ))
                })
                .collect::<Result<_>>()?,
            calendar_tokens: BTreeMap::new(),
            counts: BTreeMap::new(),
            barriers: BTreeMap::new(),
            faults: Vec::new(),
            mailboxes: ["alpha@example.test", "beta@example.test"]
                .into_iter()
                .map(|account| Ok((account.into(), super::mail_state::Mailbox::seeded(account)?)))
                .collect::<Result<_>>()?,
            pages: BTreeMap::new(),
            page_cap: 2,
            reverse: false,
            accounts: ["alpha@example.test".into(), "beta@example.test".into()].into(),
            sequence: 0,
            now: 0,
            codes: BTreeMap::new(),
            access: BTreeMap::new(),
            refresh: BTreeMap::new(),
            deny_consent: false,
            denied_scopes: BTreeSet::new(),
        })
    }
    pub fn mailbox(&self, account: &str) -> Result<&super::mail_state::Mailbox> {
        self.mailboxes
            .get(account)
            .ok_or_else(|| Reply::error(404, "notFound"))
    }
    pub fn calendar(&self, account: &str, id: &str) -> Result<&super::calendar_state::Calendar> {
        self.calendars
            .get(account)
            .and_then(|calendars| calendars.get(id))
            .ok_or_else(|| Reply::error(404, "notFound"))
    }
    pub fn calendar_mut(
        &mut self,
        account: &str,
        id: &str,
    ) -> Result<&mut super::calendar_state::Calendar> {
        self.calendars
            .get_mut(account)
            .and_then(|calendars| calendars.get_mut(id))
            .ok_or_else(|| Reply::error(404, "notFound"))
    }
    pub fn mailbox_mut(&mut self, account: &str) -> Result<&mut super::mail_state::Mailbox> {
        self.mailboxes
            .get_mut(account)
            .ok_or_else(|| Reply::error(404, "notFound"))
    }
    pub fn next(&mut self, prefix: &str) -> String {
        self.sequence += 1;
        format!("mock-{prefix}-{}", self.sequence)
    }
    pub fn record_credential(&mut self, credential: &str, account: &str) {
        use sha2::{Digest, Sha256};
        self.issued_credentials
            .insert(Sha256::digest(credential).into(), account.into());
    }
    pub fn observed_account(&self, input: &Input) -> Option<String> {
        use sha2::{Digest, Sha256};
        // Observation is independent of authorization: revoked/expired attempts
        // still count, while only authorize() can grant access to remote data.
        let fingerprint: [u8; 32] = if input.path == "/token" {
            let form = super::wire::Query::parse(&input.body);
            let credential = match form.get("grant_type") {
                Some("authorization_code") => form.get("code"),
                Some("refresh_token") => form.get("refresh_token"),
                _ => None,
            }?;
            Sha256::digest(credential).into()
        } else {
            let credential = input
                .headers
                .get("authorization")
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.strip_prefix("Bearer "))?;
            Sha256::digest(credential).into()
        };
        self.issued_credentials.get(&fingerprint).cloned()
    }
    pub fn authorize(&self, input: &Input) -> Result<String> {
        let token = input
            .headers
            .get("authorization")
            .and_then(|s| s.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(|| Reply::error(401, "authError"))?;
        let grant = self
            .access
            .get(token)
            .filter(|grant| grant.expires > self.now)
            .ok_or_else(|| Reply::error(401, "authError"))?;
        if input.path == "/v1/userinfo" {
            let scopes: BTreeSet<_> = grant.scopes.split_whitespace().collect();
            return if scopes.contains("openid") && scopes.contains("email") {
                Ok(grant.account.clone())
            } else {
                Err(Reply::error(403, "insufficientPermissions"))
            };
        }
        let needed = if input.path.starts_with("/gmail/") {
            "gmail"
        } else {
            "calendar"
        };
        let write = input.method != "GET";
        let allowed = grant.scopes.split_whitespace().any(|scope| {
            if needed == "gmail" {
                matches!(
                    scope,
                    "https://mail.google.com/" | "https://www.googleapis.com/auth/gmail.modify"
                ) || (!write
                    && matches!(
                        scope,
                        "https://www.googleapis.com/auth/gmail.readonly"
                            | "https://www.googleapis.com/auth/gmail.metadata"
                    ))
                    || (input.path.ends_with("/send")
                        && scope == "https://www.googleapis.com/auth/gmail.send")
            } else {
                scope == "https://www.googleapis.com/auth/calendar"
                    || scope == "https://www.googleapis.com/auth/calendar.events"
                    || (!write && scope == "https://www.googleapis.com/auth/calendar.readonly")
            }
        });
        if !allowed {
            return Err(Reply::error(403, "insufficientPermissions"));
        }
        if needed == "gmail"
            && !grant.scopes.split_whitespace().any(|scope| {
                matches!(
                    scope,
                    "https://mail.google.com/"
                        | "https://www.googleapis.com/auth/gmail.modify"
                        | "https://www.googleapis.com/auth/gmail.readonly"
                )
            })
        {
            let resource = input.path.contains("/messages/") || input.path.contains("/threads/");
            if input.method == "GET"
                && (input.query.get("q").is_some()
                    || input.path.contains("/attachments/")
                    || resource
                        && !matches!(input.query.get("format"), Some("metadata" | "minimal")))
            {
                return Err(Reply::error(403, "insufficientPermissions"));
            }
        }
        Ok(grant.account.clone())
    }
}
