use crate::store::StoreError;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const REQUEST_LIMIT: usize = 2;
const QUEUE_LIMIT: usize = 64;

pub(crate) struct Resources {
    requests: Arc<Semaphore>,
    admitted: Arc<Semaphore>,
    jobs: Arc<Semaphore>,
    started: AtomicU64,
    peak: AtomicU64,
    received: AtomicU64,
    batches: AtomicU64,
}
pub(crate) struct RequestPermit {
    _active: OwnedSemaphorePermit,
    _admitted: OwnedSemaphorePermit,
}

#[derive(Serialize, Default)]
pub struct ResourceStatus {
    pub requests_active: u64,
    pub requests_waiting: u64,
    pub requests_peak: u64,
    pub requests_started: u64,
    pub request_limit: u64,
    pub bytes_received: u64,
    pub background_jobs: u64,
    pub background_job_limit: u64,
    pub account_requests: u64,
    pub account_request_limit: u64,
    pub store_queue_depth: u64,
    pub store_queue_limit: u64,
    pub storage_page_batches: u64,
}
impl Resources {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: Arc::new(Semaphore::new(REQUEST_LIMIT)),
            admitted: Arc::new(Semaphore::new(QUEUE_LIMIT)),
            jobs: Arc::new(Semaphore::new(QUEUE_LIMIT)),
            started: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            received: AtomicU64::new(0),
            batches: AtomicU64::new(0),
        })
    }
    pub async fn request(&self) -> Result<RequestPermit, StoreError> {
        let admitted = self
            .admitted
            .clone()
            .try_acquire_owned()
            .map_err(|_| StoreError::Busy)?;
        let active = self
            .requests
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        self.started.fetch_add(1, Ordering::Relaxed);
        self.peak.fetch_max(
            (REQUEST_LIMIT - self.requests.available_permits()) as u64,
            Ordering::Relaxed,
        );
        Ok(RequestPermit {
            _active: active,
            _admitted: admitted,
        })
    }
    pub fn job(&self) -> Result<OwnedSemaphorePermit, StoreError> {
        self.jobs
            .clone()
            .try_acquire_owned()
            .map_err(|_| StoreError::Busy)
    }
    pub fn received(&self, bytes: usize) {
        self.received.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    pub fn page_stored(&self) {
        self.batches.fetch_add(1, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> ResourceStatus {
        let active = REQUEST_LIMIT - self.requests.available_permits();
        ResourceStatus {
            requests_active: active as u64,
            requests_waiting: (QUEUE_LIMIT - self.admitted.available_permits())
                .saturating_sub(active) as u64,
            requests_peak: self.peak.load(Ordering::Relaxed),
            requests_started: self.started.load(Ordering::Relaxed),
            request_limit: REQUEST_LIMIT as u64,
            bytes_received: self.received.load(Ordering::Relaxed),
            background_jobs: (QUEUE_LIMIT - self.jobs.available_permits()) as u64,
            background_job_limit: QUEUE_LIMIT as u64,
            storage_page_batches: self.batches.load(Ordering::Relaxed),
            ..Default::default()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use futures_util::{stream::FuturesUnordered, StreamExt};
    #[tokio::test]
    async fn budgets_bound_jobs_and_requests_and_release_cancelled_waiters() {
        let resources = Resources::new();
        let mut jobs = (0..64)
            .map(|_| resources.job().unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(resources.job(), Err(StoreError::Busy)));
        let first = resources.request().await.unwrap();
        let second = resources.request().await.unwrap();
        let mut waiting = FuturesUnordered::new();
        for _ in 0..63 {
            waiting.push(resources.request());
        }
        let rejected = tokio::time::timeout(std::time::Duration::from_secs(1), waiting.next())
            .await
            .unwrap();
        assert!(matches!(rejected, Some(Err(StoreError::Busy))));
        assert_eq!(resources.snapshot().requests_active, 2);
        assert_eq!(resources.snapshot().requests_waiting, 62);
        drop(waiting);
        drop(first);
        drop(second);
        jobs.pop();
        let replacement = resources.job().unwrap();
        let request = resources.request().await.unwrap();
        resources.received(42);
        resources.page_stored();
        assert_eq!(resources.snapshot().bytes_received, 42);
        assert_eq!(resources.snapshot().storage_page_batches, 1);
        assert_eq!(resources.snapshot().requests_started, 3);
        assert_eq!(resources.snapshot().requests_peak, 2);
        drop(request);
        drop(replacement);
        drop(jobs);
        assert_eq!(resources.snapshot().requests_active, 0);
        assert_eq!(resources.snapshot().requests_waiting, 0);
        assert_eq!(resources.snapshot().background_jobs, 0);
    }
}
