use crate::{
    accounts::{AccountError, Accounts},
    domain::calendar::{AgendaWindow, CalendarObject},
    store::{CalendarCatalogEntry, Store, StoreError, StoredCalendar, SyncRun},
    sync_error::SyncError,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio::sync::{watch, Mutex};
struct Job {
    window: AgendaWindow,
    repair: bool,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
struct EventsResult {
    cursor: Option<String>,
    time_zone: String,
}
pub(crate) struct CalendarSync {
    accounts: Arc<Accounts>,
    store: Store,
    coordinator: Arc<crate::coordination::Coordinator>,
    jobs: Mutex<BTreeMap<(String, String), Job>>,
}
impl CalendarSync {
    pub fn new(
        accounts: Arc<Accounts>,
        store: Store,
        coordinator: Arc<crate::coordination::Coordinator>,
    ) -> Arc<Self> {
        Arc::new(Self {
            accounts,
            store,
            coordinator,
            jobs: Mutex::new(BTreeMap::new()),
        })
    }
    pub async fn free_busy(
        &self,
        q: crate::domain::free_busy::FreeBusyRequest,
    ) -> Result<crate::domain::free_busy::FreeBusyResult, SyncError> {
        let body = q.provider_body()?;
        let _permit = self.coordinator.acquire(&q.account_id).await?;
        let value = self
            .accounts
            .calendar_free_busy(&q.account_id, &body)
            .await?;
        let now = self.accounts.http.clock.now_ms();
        tokio::task::spawn_blocking(move || {
            crate::domain::free_busy::FreeBusyResult::from_google(&q, value, now)
        })
        .await
        .map_err(|_| SyncError::Provider)?
    }
    pub async fn start(
        self: &Arc<Self>,
        account: String,
        window: Option<AgendaWindow>,
    ) -> Result<SyncRun, SyncError> {
        self.start_mode(account, window, false).await
    }
    pub async fn repair(
        self: &Arc<Self>,
        account: String,
        window: AgendaWindow,
    ) -> Result<SyncRun, SyncError> {
        self.start_mode(account, Some(window), true).await
    }
    async fn start_mode(
        self: &Arc<Self>,
        account: String,
        window: Option<AgendaWindow>,
        repair: bool,
    ) -> Result<SyncRun, SyncError> {
        let row = self
            .store
            .account(account.clone())
            .await?
            .ok_or(StoreError::NotFound)?;
        if row.account.provider != "google" {
            return Err(AccountError::Invalid.into());
        }
        if row.state != "connected" {
            return Err(AccountError::Authorization.into());
        }
        let window = window
            .unwrap_or(AgendaWindow::rolling(self.now()?).map_err(|_| StoreError::InvalidInput)?);
        AgendaWindow::new(&window.from, &window.to).map_err(|_| StoreError::InvalidInput)?;
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|_, j| !j.task.is_finished());
        if let Some(((.., id), job)) = jobs.iter().find(|((a, _), _)| a == &account) {
            if job.window == window && (!repair || job.repair) {
                return Ok(self.store.sync_run(account, id.clone()).await?);
            }
            return Err(StoreError::Busy.into());
        }
        let run = self
            .store
            .start_calendar_run(account.clone(), self.now()?)
            .await?;
        let service = self.clone();
        let work_run = run.clone();
        let job_window = window.clone();
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let work_stop = stopped.clone();
            let work = async {
                let _permit = service
                    .coordinator
                    .acquire_sync(
                        &service.store,
                        &service.accounts.http.clock,
                        &work_run.account_id,
                        "calendar",
                    )
                    .await?;
                service
                    .synchronize(&work_run, window, repair, work_stop)
                    .await
            };
            let result =
                tokio::select! {result=work=>result,_=stopped.changed()=>Err(SyncError::Cancelled)};
            if let Err(e) = result {
                let state = if matches!(e, SyncError::Cancelled) {
                    "cancelled"
                } else {
                    "failed"
                };
                let _ = service
                    .store
                    .finish_sync_run_error(
                        work_run.account_id,
                        work_run.id,
                        state.into(),
                        e.code().into(),
                        service.now().unwrap_or(work_run.started_at_ms),
                    )
                    .await;
            }
        });
        jobs.insert(
            (account, run.id.clone()),
            Job {
                stop,
                task,
                window: job_window,
                repair,
            },
        );
        Ok(run)
    }
    async fn synchronize(
        &self,
        run: &SyncRun,
        window: AgendaWindow,
        repair: bool,
        stop: watch::Receiver<bool>,
    ) -> Result<(), SyncError> {
        self.store
            .begin_sync_run(run.account_id.clone(), run.id.clone(), None)
            .await?;
        let mut page: Option<String> = None;
        let mut seen = BTreeSet::new();
        loop {
            let mut query = vec![("maxResults", "100")];
            if let Some(page) = &page {
                query.push(("pageToken", page));
            }
            let response = self
                .accounts
                .calendar_get(
                    &run.account_id,
                    &["users", "me", "calendarList"],
                    &query,
                    4 * 1024 * 1024,
                )
                .await?;
            for value in items(&response)? {
                let access = optional(value, "accessRole")?.unwrap_or_else(|| "none".into());
                self.store
                    .stage_calendar_entry(
                        run.account_id.clone(),
                        run.id.clone(),
                        CalendarCatalogEntry {
                            provider_id: required(value, "id")?,
                            summary: optional(value, "summary")?,
                            time_zone: optional(value, "timeZone")?,
                            access_role: access,
                            is_primary: value
                                .get("primary")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            provider_json: serde_json::to_string(value)
                                .map_err(|_| SyncError::Provider)?,
                        },
                    )
                    .await?;
            }
            page = next(&response, &mut seen)?;
            if page.is_none() {
                break;
            }
        }
        if repair {
            return self.repair_staged(run, window, stop).await;
        }
        self.store
            .promote_calendar_catalog(run.account_id.clone(), run.id.clone())
            .await?;
        let mut processed = 0;
        for calendar in self
            .store
            .calendars(run.account_id.clone())
            .await?
            .into_iter()
            .filter(|c| !c.retired)
        {
            if matches!(calendar.access_role.as_str(), "none" | "freeBusyReader") {
                continue;
            }
            self.canonical(run, &calendar, &mut processed).await?;
            // Capture the canonical revision after its promotion; another change
            // must not label an older expanded window current.
            let refreshed = self
                .store
                .calendars(run.account_id.clone())
                .await?
                .into_iter()
                .find(|c| c.id == calendar.id)
                .ok_or(StoreError::NotFound)?;
            self.occurrences(run, &refreshed, &window, &mut processed)
                .await?;
        }
        self.store
            .finish_calendar_run(run.account_id.clone(), run.id.clone(), self.now()?)
            .await?;
        Ok(())
    }
    async fn repair_staged(
        &self,
        run: &SyncRun,
        window: AgendaWindow,
        _stop: watch::Receiver<bool>,
    ) -> Result<(), SyncError> {
        let calendars = self
            .store
            .calendar_repair_targets(run.account_id.clone(), run.id.clone())
            .await?;
        let mut checkpoints = Vec::new();
        let mut processed = 0;
        for mut calendar in calendars {
            if matches!(calendar.access_role.as_str(), "none" | "freeBusyReader") {
                continue;
            }
            let canonical = self
                .events(run, &calendar, None, None, &mut processed, true)
                .await?;
            calendar.time_zone = Some(canonical.time_zone.clone());
            let expanded = self
                .events(run, &calendar, None, Some(&window), &mut processed, true)
                .await?;
            if canonical.time_zone != expanded.time_zone {
                return Err(SyncError::Provider);
            }
            checkpoints.push(crate::store::CalendarRepairCheckpoint {
                calendar_id: calendar.id,
                cursor: canonical.cursor.ok_or(SyncError::Provider)?,
                time_zone: canonical.time_zone,
            });
        }
        #[cfg(feature = "test-harness")]
        if let Some(config) = &self.accounts.http.test_config {
            config
                .checkpoint("calendar-repair-before-promotion", _stop)
                .await
                .map_err(|_| SyncError::Cancelled)?;
        }
        self.store
            .promote_calendar_repair(
                run.account_id.clone(),
                run.id.clone(),
                window,
                checkpoints,
                self.now()?,
            )
            .await?;
        Ok(())
    }
    async fn canonical(
        &self,
        run: &SyncRun,
        calendar: &StoredCalendar,
        processed: &mut u64,
    ) -> Result<(), SyncError> {
        let mut cursor = self
            .store
            .calendar_cursor(run.account_id.clone(), calendar.id.clone())
            .await?;
        for attempt in 0..2 {
            let result = self
                .events(run, calendar, cursor.as_deref(), None, processed, false)
                .await;
            match result {
                Ok(result) => {
                    if cursor.is_some() && calendar.time_zone.as_deref() != Some(&result.time_zone)
                    {
                        if attempt != 0 {
                            return Err(SyncError::Provider);
                        }
                        self.store
                            .clear_calendar_staging(
                                run.account_id.clone(),
                                run.id.clone(),
                                calendar.id.clone(),
                                false,
                            )
                            .await?;
                        cursor = None;
                        continue;
                    }
                    self.store
                        .promote_calendar_objects(
                            run.account_id.clone(),
                            run.id.clone(),
                            calendar.id.clone(),
                            crate::store::CalendarCheckpoint {
                                cursor: result.cursor.ok_or(SyncError::Provider)?,
                                full: cursor.is_none(),
                                time_zone: result.time_zone,
                            },
                            self.now()?,
                        )
                        .await?;
                    return Ok(());
                }
                Err(SyncError::CalendarExpired) if attempt == 0 => {
                    self.store
                        .clear_calendar_staging(
                            run.account_id.clone(),
                            run.id.clone(),
                            calendar.id.clone(),
                            false,
                        )
                        .await?;
                    cursor = None;
                }
                Err(e) => return Err(e),
            }
        }
        Err(SyncError::CalendarExpired)
    }
    async fn occurrences(
        &self,
        run: &SyncRun,
        calendar: &StoredCalendar,
        window: &AgendaWindow,
        processed: &mut u64,
    ) -> Result<(), SyncError> {
        let result = self
            .events(run, calendar, None, Some(window), processed, false)
            .await?;
        if calendar.time_zone.as_deref() != Some(&result.time_zone) {
            return Err(SyncError::Provider);
        }
        self.store
            .promote_calendar_occurrences(
                run.account_id.clone(),
                run.id.clone(),
                calendar.id.clone(),
                window.clone(),
                calendar.canonical_revision,
                self.now()?,
            )
            .await?;
        Ok(())
    }
    async fn events(
        &self,
        run: &SyncRun,
        calendar: &StoredCalendar,
        cursor: Option<&str>,
        window: Option<&AgendaWindow>,
        processed: &mut u64,
        repair: bool,
    ) -> Result<EventsResult, SyncError> {
        let bounds = window
            .map(|w| {
                w.provider_bounds(calendar.time_zone.as_deref().ok_or(SyncError::Provider)?)
                    .map_err(|_| SyncError::Provider)
            })
            .transpose()?;
        let mut page: Option<String> = None;
        let mut seen = BTreeSet::new();
        let mut response_zone: Option<String> = None;
        loop {
            let mut query = vec![
                ("maxResults", "100"),
                (
                    "singleEvents",
                    if window.is_some() { "true" } else { "false" },
                ),
                ("showDeleted", "true"),
            ];
            if let Some(token) = cursor {
                query.push(("syncToken", token));
            }
            if let Some(page) = &page {
                query.push(("pageToken", page));
            }
            if let Some((from, to)) = &bounds {
                query.extend([("timeMin", from.as_str()), ("timeMax", to.as_str())]);
            }
            let response = self
                .accounts
                .calendar_get(
                    &run.account_id,
                    &["calendars", &calendar.provider_id, "events"],
                    &query,
                    32 * 1024 * 1024,
                )
                .await?;
            let page_zone = optional(&response, "timeZone")?;
            let zone = page_zone
                .as_deref()
                .or(calendar.time_zone.as_deref())
                .ok_or(SyncError::Provider)?;
            if zone.parse::<chrono_tz::Tz>().is_err()
                || response_zone
                    .as_deref()
                    .is_some_and(|previous| previous != zone)
            {
                return Err(SyncError::Provider);
            }
            response_zone = Some(zone.to_owned());
            for value in items(&response)? {
                let object = CalendarObject::from_google(value.clone(), zone)
                    .map_err(|_| SyncError::Provider)?;
                if repair {
                    self.store
                        .stage_calendar_repair_event(
                            run.account_id.clone(),
                            run.id.clone(),
                            calendar.id.clone(),
                            window.is_some(),
                            object,
                        )
                        .await?;
                } else {
                    self.store
                        .stage_calendar_event(
                            run.account_id.clone(),
                            run.id.clone(),
                            calendar.id.clone(),
                            window.is_some(),
                            object,
                        )
                        .await?;
                }
                *processed += 1;
            }
            page = next(&response, &mut seen)?;
            self.store
                .sync_run_progress(
                    run.account_id.clone(),
                    run.id.clone(),
                    *processed,
                    page.clone(),
                )
                .await?;
            if page.is_none() {
                return Ok(EventsResult {
                    time_zone: response_zone.ok_or(SyncError::Provider)?,
                    cursor: if window.is_some() {
                        None
                    } else {
                        Some(required(&response, "nextSyncToken")?)
                    },
                });
            }
        }
    }
    pub async fn cancel(&self, account: &str, id: &str) -> Result<SyncRun, SyncError> {
        self.store.sync_run(account.into(), id.into()).await?;
        if let Some(job) = self.jobs.lock().await.remove(&(account.into(), id.into())) {
            job.stop.send_replace(true);
            job.task.await.map_err(|_| SyncError::Unavailable)?;
        }
        Ok(self.store.sync_run(account.into(), id.into()).await?)
    }
    pub async fn shutdown(&self) {
        let jobs = std::mem::take(&mut *self.jobs.lock().await);
        for job in jobs.values() {
            job.stop.send_replace(true);
        }
        for (_, job) in jobs {
            let _ = job.task.await;
        }
    }
    fn now(&self) -> Result<i64, StoreError> {
        Ok(self.accounts.http.clock.now_ms())
    }
}
fn items(value: &Value) -> Result<&[Value], SyncError> {
    match value.get("items") {
        None => Ok(&[]),
        Some(Value::Array(a)) if a.len() <= 2500 => Ok(a),
        _ => Err(SyncError::Provider),
    }
}
fn optional(value: &Value, key: &str) -> Result<Option<String>, SyncError> {
    match value.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if s.len() <= 8192 => Ok(Some(s.clone())),
        _ => Err(SyncError::Provider),
    }
}
fn required(value: &Value, key: &str) -> Result<String, SyncError> {
    optional(value, key)?
        .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
        .ok_or(SyncError::Provider)
}
fn next(value: &Value, seen: &mut BTreeSet<String>) -> Result<Option<String>, SyncError> {
    let token = optional(value, "nextPageToken")?;
    if let Some(token) = &token {
        if token.is_empty() || seen.len() >= 100_000 || !seen.insert(token.clone()) {
            return Err(SyncError::Provider);
        }
    }
    Ok(token)
}
