use crate::{
    accounts::AccountError,
    calendar::CalendarSync,
    mail::MailSync,
    store::{Store, StoreError},
    sync_error::SyncError,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::watch, time::Instant};

pub(crate) struct Scheduler {
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
    pub enabled: bool,
    error: watch::Receiver<Option<String>>,
}
impl Scheduler {
    pub async fn start(
        store: Store,
        mail: Arc<MailSync>,
        calendar: Arc<CalendarSync>,
    ) -> Result<Self, StoreError> {
        let (poll, enabled) = mail.accounts.http.sync_policy();
        store.configure_poll_interval(poll).await?;
        let (stop, stopped) = watch::channel(false);
        let (failed, error) = watch::channel(None);
        let task = enabled.then(|| {
            tokio::spawn(async move {
                if let Err(error) = run(store, mail, calendar, stopped, poll).await {
                    failed.send_replace(Some(error.code().into()));
                }
            })
        });
        Ok(Self {
            stop,
            task,
            enabled,
            error,
        })
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
impl Drop for Scheduler {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
struct Deadline {
    wall_ms: i64,
    at: Option<Instant>,
}
async fn run(
    store: Store,
    mail: Arc<MailSync>,
    calendar: Arc<CalendarSync>,
    mut stop: watch::Receiver<bool>,
    poll: u64,
) -> Result<(), SyncError> {
    let clock = mail.accounts.http.clock.clone();
    let mut tick = tokio::time::interval(Duration::from_millis(poll.min(1000)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut deadlines: BTreeMap<(String, String), Deadline> = BTreeMap::new();
    let mut previous_wall = clock.now_ms();
    let mut previous_tick = Instant::now();
    loop {
        tokio::select! {_=stop.changed()=>return Ok(()),_=tick.tick()=>{}}
        if *stop.borrow() {
            return Ok(());
        }
        #[cfg(feature = "test-harness")]
        mail.accounts.http.refresh_test_clock().await?;
        let now = clock.now_ms();
        let instant = Instant::now();
        let elapsed =
            i64::try_from(instant.duration_since(previous_tick).as_millis()).unwrap_or(i64::MAX);
        // Rust does not guarantee that Instant includes suspend time. Reconcile
        // persisted wall deadlines after a forward wall/monotonic discrepancy.
        if now.saturating_sub(previous_wall) > elapsed.saturating_add(1000) {
            deadlines.clear();
        }
        previous_wall = now;
        previous_tick = instant;
        let schedules = store.sync_schedules().await?;
        let keys: BTreeSet<_> = schedules
            .iter()
            .map(|s| (s.account_id.clone(), s.scope.clone()))
            .collect();
        deadlines.retain(|key, _| keys.contains(key));
        for scope in schedules {
            if scope.account_state != "connected"
                || matches!(scope.run_state.as_deref(), Some("queued" | "running"))
            {
                continue;
            }
            let key = (scope.account_id.clone(), scope.scope.clone());
            let entry = deadlines.entry(key).or_insert(Deadline {
                wall_ms: i64::MIN,
                at: None,
            });
            if entry.wall_ms != scope.next_attempt_at_ms {
                let wait = u64::try_from(scope.next_attempt_at_ms.saturating_sub(now).max(0))
                    .map_err(|_| StoreError::InvalidInput)?;
                *entry = Deadline {
                    wall_ms: scope.next_attempt_at_ms,
                    at: instant.checked_add(Duration::from_millis(wait)),
                };
            }
            if entry.at.is_none_or(|deadline| deadline > instant) {
                continue;
            }
            let started = match scope.scope.as_str() {
                "gmail" | "imap" => mail.start(scope.account_id, false, None).await,
                "calendar" => calendar.start(scope.account_id, None).await,
                _ => continue,
            };
            match started {
                Ok(_) => {}
                Err(
                    SyncError::Storage(StoreError::Busy)
                    | SyncError::Account(AccountError::Authorization | AccountError::ScopeDenied),
                ) => {}
                Err(error) => return Err(error),
            }
        }
    }
}
