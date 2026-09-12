mod calendar_change;
mod imap_flags;
mod imap_transfer;
mod mail_change;
mod smtp;
use crate::{
    accounts::{AccountError, Accounts},
    coordination::Coordinator,
    providers::google::send::WriteResult,
    store::{AttemptKind, AttemptOutcome, OperationReceipt, Store, StoreError},
};
use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{
    sync::{watch, Semaphore},
    task::JoinSet,
};

pub(crate) struct OperationWorker {
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
    error: watch::Receiver<Option<String>>,
}
enum PreparedWrite {
    Smtp {
        _memory: tokio::sync::OwnedSemaphorePermit,
        raw: Vec<u8>,
        payload: crate::store::SendPayload,
        intent: Box<crate::store::SmtpIntent>,
    },
    Send {
        memory: tokio::sync::OwnedSemaphorePermit,
        raw: Vec<u8>,
        payload: crate::store::SendPayload,
    },
    Change(crate::store::MailChangePayload),
    ImapFlags(crate::store::ImapFlagPayload),
    ImapTransfer {
        payload: crate::store::ImapTransferPayload,
        _memory: tokio::sync::OwnedSemaphorePermit,
    },
    Calendar(crate::store::CalendarChangePayload),
}
struct Service {
    store: Store,
    accounts: Arc<Accounts>,
    coordinator: Arc<Coordinator>,
    composition: Arc<Semaphore>,
}
impl OperationWorker {
    pub fn start(
        store: Store,
        accounts: Arc<Accounts>,
        coordinator: Arc<Coordinator>,
        composition: Arc<Semaphore>,
    ) -> Self {
        let (stop, _) = watch::channel(false);
        let (failed, error) = watch::channel(None);
        let service = Arc::new(Service {
            store,
            accounts,
            coordinator,
            composition,
        });
        let stopping = stop.clone();
        let task = tokio::spawn(async move {
            if run(service, stopping).await.is_err() {
                failed.send_replace(Some("operation_worker_unavailable".into()));
            }
        });
        Self {
            stop,
            task: Some(task),
            error,
        }
    }
    pub fn error(&self) -> Option<String> {
        self.error.borrow().clone()
    }
    pub async fn shutdown(mut self) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for OperationWorker {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
async fn run(service: Arc<Service>, stop: watch::Sender<bool>) -> Result<(), StoreError> {
    let mut stopped = stop.subscribe();
    let mut jobs = JoinSet::new();
    let mut accounts = BTreeSet::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop {
        if *stopped.borrow() {
            break Ok(());
        }
        tokio::select! {
            _=stopped.changed()=>break Ok(()),
            finished=jobs.join_next(),if !jobs.is_empty()=>{
                match finished {
                    Some(Ok((account,Ok(()))))=>{accounts.remove(&account);},
                    _=>break Err(StoreError::Unavailable),
                }
            },
            _=tick.tick()=>{
                #[cfg(feature="test-harness")]
                if service.accounts.http.refresh_test_clock().await.is_err() { break Err(StoreError::Unavailable); }
                let pending=match service.store.ready_operations(service.accounts.http.clock.now_ms()).await {
                    Ok(pending)=>pending,Err(error)=>break Err(error),
                };
                for work in pending {
                    if !accounts.insert(work.account_id.clone()) { continue; }
                    let service=service.clone();let stopped=stop.subscribe();
                    jobs.spawn(async move {
                        let account=work.account_id.clone();
                        let result=process(service.clone(),work,stopped.clone()).await;
                        let _=service.checkpoint("operation_job_finished",stopped).await;
                        (account,result)
                    });
                }
            }
        }
    };
    stop.send_replace(true);
    while jobs.join_next().await.is_some() {}
    result
}
async fn process(
    service: Arc<Service>,
    work: crate::store::ReadyOperation,
    mut stop: watch::Receiver<bool>,
) -> Result<(), StoreError> {
    let preparation_stop = stop.clone();
    let preparation = async {
        service
            .checkpoint("operation_before_dispatch", preparation_stop)
            .await?;
        let imap = service
            .store
            .account(work.account_id.clone())
            .await?
            .ok_or(StoreError::NotFound)?
            .account
            .provider
            == "imap";
        let lane = service
            .coordinator
            .acquire_sync(
                &service.store,
                &service.accounts.http.clock,
                &work.account_id,
                if imap {
                    "imap"
                } else if work.kind == "calendar_change" {
                    "calendar"
                } else {
                    "gmail"
                },
            )
            .await?;
        let prepared = if work.kind == "calendar_change" {
            PreparedWrite::Calendar(
                service
                    .store
                    .calendar_change_payload(work.account_id.clone(), work.id.clone())
                    .await?,
            )
        } else if work.kind == "mail_change" && imap {
            match service
                .store
                .imap_change_payload(work.account_id.clone(), work.id.clone())
                .await?
            {
                crate::store::ImapChangePayload::Flags(payload) => {
                    PreparedWrite::ImapFlags(payload)
                }
                crate::store::ImapChangePayload::Transfer(payload) => PreparedWrite::ImapTransfer {
                    payload,
                    _memory: service
                        .composition
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| StoreError::Unavailable)?,
                },
            }
        } else if work.kind == "mail_change" {
            PreparedWrite::Change(
                service
                    .store
                    .mail_change_payload(work.account_id.clone(), work.id.clone())
                    .await?,
            )
        } else {
            let memory = service
                .composition
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| StoreError::Unavailable)?;
            let payload = service
                .store
                .send_payload(work.account_id.clone(), work.id.clone())
                .await?;
            let raw = service
                .store
                .read_blob(work.account_id.clone(), payload.wire.clone())
                .await?;
            if imap {
                let intent = service
                    .store
                    .smtp_intent(work.account_id.clone(), work.id.clone())
                    .await?;
                PreparedWrite::Smtp {
                    _memory: memory,
                    raw,
                    payload,
                    intent: Box::new(intent),
                }
            } else {
                PreparedWrite::Send {
                    memory,
                    raw,
                    payload,
                }
            }
        };
        Ok::<_, StoreError>((lane, prepared))
    };
    let (_lane, prepared) = tokio::select! {
        result=preparation=>match result {
            Ok(prepared)=>prepared,
            // No attempt has started; leave durable intent ready for the next tick.
            Err(StoreError::Busy)=>return Ok(()),
            Err(error)=>return Err(error),
        },
        _=stop.changed()=>return Ok(()),
    };
    if *stop.borrow() {
        return Ok(());
    }
    let kind = if work.state == "uncertain" {
        AttemptKind::Reconcile
    } else {
        AttemptKind::Dispatch
    };
    let attempt = match service
        .store
        .begin_operation_attempt(
            work.account_id.clone(),
            work.id.clone(),
            kind,
            service.accounts.http.clock.now_ms(),
        )
        .await
    {
        Ok(attempt) => attempt,
        Err(StoreError::VersionConflict | StoreError::Busy) => return Ok(()),
        Err(error) => return Err(error),
    };
    let interrupted = || AttemptOutcome::Uncertain {
        code: "interrupted".into(),
    };
    let policy = service
        .store
        .operation_execution_policy(work.account_id.clone(), work.id.clone())
        .await?;
    let retry_ordinal = policy.retry_ordinal(attempt.ordinal)?;
    let outcome = if *stop.borrow() {
        interrupted()
    } else {
        let work_stop = stop.clone();
        let work_call = async {
            service
                .checkpoint("operation_after_attempt", work_stop.clone())
                .await?;
            let result = match prepared {
                PreparedWrite::Smtp {
                    _memory,
                    raw,
                    payload,
                    intent,
                } => {
                    service
                        .smtp(
                            &work.id,
                            &intent,
                            &payload,
                            &raw,
                            (kind, attempt.ordinal, policy),
                            work_stop.clone(),
                        )
                        .await?
                }
                PreparedWrite::Calendar(payload) => {
                    service
                        .change_calendar(&work.account_id, &payload, kind, retry_ordinal)
                        .await?
                }
                PreparedWrite::ImapTransfer { payload, _memory } => {
                    service
                        .transfer(
                            &work.account_id,
                            &work.id,
                            &payload,
                            (kind, attempt.ordinal, policy),
                            work_stop.clone(),
                        )
                        .await?
                }
                PreparedWrite::ImapFlags(payload) => {
                    service
                        .imap_flags(&work.account_id, &payload, kind, retry_ordinal)
                        .await?
                }
                PreparedWrite::Change(payload) => {
                    service
                        .change(&work.account_id, &payload, kind, retry_ordinal)
                        .await?
                }
                PreparedWrite::Send {
                    memory,
                    raw,
                    payload,
                } => {
                    if kind == AttemptKind::Dispatch {
                        let (body, _memory) = tokio::task::spawn_blocking(move || {
                            let mut body = serde_json::json!({"raw":URL_SAFE_NO_PAD.encode(raw)});
                            if let Some(id) = payload.thread_id {
                                body["threadId"] = id.into();
                            }
                            (serde_json::to_vec(&body), memory)
                        })
                        .await
                        .map_err(|_| StoreError::Unavailable)?;
                        service
                            .send(
                                &work.account_id,
                                &payload.sender,
                                body.map_err(|_| StoreError::InvalidInput)?,
                                retry_ordinal,
                            )
                            .await?
                    } else {
                        let _memory = memory;
                        service
                            .reconcile(&work.account_id, &payload.message_id, &raw)
                            .await?
                    }
                }
            };
            service
                .checkpoint("operation_before_receipt", work_stop.clone())
                .await?;
            Ok::<_, StoreError>(result)
        };
        tokio::select! {
            result=work_call=>match result {
                Ok(outcome)=>outcome,
                Err(_)=>AttemptOutcome::Uncertain{code:"operation_processing_failed".into()},
            },
            _=stop.changed()=>interrupted(),
            _=tokio::time::sleep(Duration::from_secs(60))=>AttemptOutcome::Uncertain{code:"operation_deadline".into()},
        }
    };
    service
        .store
        .finish_operation_attempt(
            work.account_id,
            work.id,
            attempt.ordinal,
            outcome,
            service.accounts.http.clock.now_ms(),
        )
        .await?;
    Ok(())
}
impl Service {
    async fn checkpoint(
        &self,
        _name: &str,
        _stop: watch::Receiver<bool>,
    ) -> Result<(), StoreError> {
        #[cfg(feature = "test-harness")]
        if let Some(config) = &self.accounts.http.test_config {
            config
                .checkpoint(_name, _stop)
                .await
                .map_err(|_| StoreError::Unavailable)?;
        }
        Ok(())
    }
    async fn send(
        &self,
        account: &str,
        sender: &str,
        body: Vec<u8>,
        ordinal: u32,
    ) -> Result<AttemptOutcome, StoreError> {
        let now = self.accounts.http.clock.now_ms();
        let base = (1000_i64 << ordinal.saturating_sub(1).min(8)).min(240_000);
        let jitter = i64::from(rand::random::<u32>()) % (base / 4 + 1);
        let retry = now.saturating_add(base + jitter);
        let result = self
            .accounts
            .gmail_write(account, Some(sender), &["messages", "send"], body, retry)
            .await;
        Ok(match result {
            Ok(WriteResult::Acknowledged(value)) => {
                let provider_id = value["id"]
                    .as_str()
                    .ok_or(StoreError::InvalidInput)?
                    .to_owned();
                AttemptOutcome::Applied(OperationReceipt {
                    kind: "google_send".into(),
                    source: "acknowledgement".into(),
                    provider_id: Some(provider_id),
                    etag: None,
                })
            }
            Ok(WriteResult::Rejected {
                code,
                retry_at_ms,
                authorization,
            }) => AttemptOutcome::Rejected {
                code: code.into(),
                retry_at_ms: if ordinal < 8 || authorization {
                    retry_at_ms
                } else {
                    None
                },
            },
            Ok(WriteResult::Uncertain { code, .. }) => {
                AttemptOutcome::Uncertain { code: code.into() }
            }
            Err(error) => AttemptOutcome::Rejected {
                code: error.code().into(),
                retry_at_ms: match error {
                    AccountError::RetryAfter(at) => Some(at),
                    AccountError::Authorization | AccountError::ScopeDenied => Some(retry),
                    AccountError::Unavailable | AccountError::Secret if ordinal < 8 => Some(retry),
                    _ => None,
                },
            },
        })
    }
    async fn reconcile(
        &self,
        account: &str,
        message_id: &str,
        original: &[u8],
    ) -> Result<AttemptOutcome, StoreError> {
        let result = self.find_sent(account, message_id, original).await;
        Ok(match result {
            Ok(Some(id)) => AttemptOutcome::Applied(OperationReceipt {
                kind: "google_send".into(),
                source: "positive_read".into(),
                provider_id: Some(id),
                etag: None,
            }),
            _ => AttemptOutcome::Uncertain {
                code: "no_positive_send_evidence".into(),
            },
        })
    }
    async fn find_sent(
        &self,
        account: &str,
        message_id: &str,
        original: &[u8],
    ) -> Result<Option<String>, crate::mail::MailError> {
        use crate::mail::MailError;
        let search = format!("rfc822msgid:{message_id}");
        let mut token = None;
        let mut pages = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut found = None;
        for _ in 0..100 {
            let mut query = vec![
                ("q", search.as_str()),
                ("labelIds", "SENT"),
                ("includeSpamTrash", "true"),
                ("maxResults", "100"),
            ];
            if let Some(token) = token.as_deref() {
                query.push(("pageToken", token));
            }
            let page = self
                .accounts
                .gmail_get(account, &["messages"], &query, 2 * 1024 * 1024)
                .await?;
            let messages = match page.get("messages") {
                Some(value) => value.as_array().ok_or(MailError::Provider)?.as_slice(),
                None => &[],
            };
            if messages.len() > 500 {
                return Err(MailError::TooLarge);
            }
            for message in messages {
                let id = message["id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 2048)
                    .ok_or(MailError::Provider)?;
                if !ids.insert(id.to_owned()) {
                    continue;
                }
                if ids.len() > 1000 {
                    return Err(MailError::TooLarge);
                }
                let limit = self.accounts.http.max_payload_bytes.saturating_mul(4) / 3 + 65536;
                let remote = self
                    .accounts
                    .gmail_get(account, &["messages", id], &[("format", "raw")], limit)
                    .await?;
                if remote["id"] != id
                    || !remote["labelIds"]
                        .as_array()
                        .is_some_and(|labels| labels.iter().any(|l| l == "SENT"))
                {
                    continue;
                }
                let encoded = remote["raw"].as_str().ok_or(MailError::Provider)?;
                let bytes = URL_SAFE_NO_PAD
                    .decode(encoded)
                    .or_else(|_| URL_SAFE.decode(encoded))
                    .map_err(|_| MailError::Provider)?;
                if bytes.len() > self.accounts.http.max_payload_bytes {
                    return Err(MailError::TooLarge);
                }
                if matching_sent(original, &bytes, message_id) {
                    if found.is_some() {
                        return Ok(None);
                    }
                    found = Some(id.into());
                }
            }
            match page.get("nextPageToken") {
                None => return Ok(found),
                Some(value) => {
                    let next = value
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 4096)
                        .ok_or(MailError::Provider)?
                        .to_owned();
                    if !pages.insert(next.clone()) {
                        return Err(MailError::Provider);
                    }
                    token = Some(next);
                }
            }
        }
        Err(MailError::TooLarge)
    }
}
fn matching_sent(original: &[u8], remote: &[u8], message_id: &str) -> bool {
    use mail_parser::HeaderName;
    let parser = mail_parser::MessageParser::default();
    let (Some(a), Some(b)) = (parser.parse_headers(original), parser.parse_headers(remote)) else {
        return false;
    };
    if b.message_id() != Some(message_id) {
        return false;
    }
    for name in [
        HeaderName::MessageId,
        HeaderName::From,
        HeaderName::To,
        HeaderName::Cc,
        HeaderName::Bcc,
        HeaderName::Subject,
        HeaderName::ContentType,
        HeaderName::ContentTransferEncoding,
    ] {
        if !a
            .headers()
            .iter()
            .filter(|h| h.name == name)
            .map(|h| &h.value)
            .eq(b
                .headers()
                .iter()
                .filter(|h| h.name == name)
                .map(|h| &h.value))
        {
            return false;
        }
    }
    let body = |raw: &[u8]| raw.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    match (body(original), body(remote)) {
        (Some(a), Some(b)) => original[a..] == remote[b..],
        _ => false,
    }
}
