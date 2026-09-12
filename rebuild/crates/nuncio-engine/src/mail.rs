use crate::{
    accounts::{AccountError, Accounts},
    store::{Store, StoreError, SyncRun},
};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{watch, Mutex};

pub use crate::sync_error::SyncError as MailError;
struct Job {
    mode: &'static str,
    fetch: Option<String>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
pub(crate) struct MailSync {
    pub accounts: Arc<Accounts>,
    pub store: Store,
    pub coordinator: Arc<crate::coordination::Coordinator>,
    jobs: Mutex<BTreeMap<(String, String), Job>>,
}
impl MailSync {
    pub async fn new(accounts: Arc<Accounts>, store: Store) -> Result<Arc<Self>, StoreError> {
        store
            .recover_sync_runs(accounts.http.clock.now_ms())
            .await?;
        Ok(Arc::new(Self {
            accounts,
            store,
            coordinator: crate::coordination::Coordinator::new(),
            jobs: Mutex::new(BTreeMap::new()),
        }))
    }
    pub async fn start(
        self: &Arc<Self>,
        account: String,
        force_full: bool,
        fetch: Option<String>,
    ) -> Result<SyncRun, MailError> {
        let row = self
            .store
            .account(account.clone())
            .await?
            .ok_or(StoreError::NotFound)?;
        if !matches!(row.account.provider.as_str(), "google" | "imap") {
            return Err(AccountError::Invalid.into());
        }
        if row.state != "connected" {
            return Err(AccountError::Authorization.into());
        }
        let cursor = self.store.mail_coverage(account.clone()).await?.cursor;
        let mode = if fetch.is_some() {
            "fetch"
        } else if force_full || cursor.is_none() {
            "full"
        } else {
            "delta"
        };
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|_, j| !j.task.is_finished());
        if let Some(((.., id), job)) = jobs.iter().find(|((a, _), _)| a == &account) {
            if job.fetch == fetch && (!force_full || job.mode == "full") {
                return Ok(self.store.sync_run(account, id.clone()).await?);
            }
            return Err(StoreError::Busy.into());
        }
        let admission = self.accounts.http.resources.job()?;
        let run = self
            .store
            .start_mail_run(account.clone(), mode.into(), self.now()?)
            .await?;
        let (stop, mut stopped) = watch::channel(false);
        let service = self.clone();
        let work_run = run.clone();
        let job_fetch = fetch.clone();
        let task = tokio::spawn(async move {
            let _admission = admission;
            let work_stop = stopped.clone();
            let work = async {
                let _permit = service
                    .coordinator
                    .acquire_sync(
                        &service.store,
                        &service.accounts.http.clock,
                        &work_run.account_id,
                        &work_run.scope,
                    )
                    .await?;
                if work_run.scope == "imap" {
                    if let Some(message) = fetch {
                        crate::providers::imap::sync::fetch(&service, &work_run, message, work_stop)
                            .await
                    } else {
                        crate::providers::imap::sync::sync(&service, &work_run, work_stop).await
                    }
                } else {
                    crate::providers::google::gmail::sync(
                        &service, &work_run, cursor, fetch, work_stop,
                    )
                    .await
                }
            };
            let result =
                tokio::select! {result=work=>result,_=stopped.changed()=>Err(MailError::Cancelled)};
            if let Err(error) = result {
                let state = if matches!(error, MailError::Cancelled) {
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
                        error.code().into(),
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
                mode,
                fetch: job_fetch,
            },
        );
        Ok(run)
    }
    pub async fn cancel(&self, account: &str, run: &str) -> Result<SyncRun, MailError> {
        self.store.sync_run(account.into(), run.into()).await?;
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.remove(&(account.into(), run.into())) {
            job.stop.send_replace(true);
            job.task.await.map_err(|_| MailError::Unavailable)?;
        }
        Ok(self.store.sync_run(account.into(), run.into()).await?)
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
    pub fn now(&self) -> Result<i64, StoreError> {
        Ok(self.accounts.http.clock.now_ms())
    }
    pub async fn checkpoint(
        &self,
        _name: &str,
        _stop: watch::Receiver<bool>,
    ) -> Result<(), MailError> {
        #[cfg(feature = "test-harness")]
        if let Some(config) = &self.accounts.http.test_config {
            config
                .checkpoint(_name, _stop)
                .await
                .map_err(|_| MailError::Cancelled)?;
        }
        Ok(())
    }
}
