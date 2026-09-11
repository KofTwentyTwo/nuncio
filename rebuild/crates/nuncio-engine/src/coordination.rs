use crate::store::StoreError;
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

pub(crate) struct Coordinator {
    active: Arc<Semaphore>,
    accounts: Mutex<BTreeMap<String, Weak<Mutex<()>>>>,
}
pub(crate) struct AccountPermit {
    _active: OwnedSemaphorePermit,
    _sequence: OwnedMutexGuard<()>,
}
impl Coordinator {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            active: Arc::new(Semaphore::new(2)),
            accounts: Mutex::new(BTreeMap::new()),
        })
    }
    pub async fn acquire_sync(
        &self,
        store: &crate::store::Store,
        clock: &crate::clock::Clock,
        account: &str,
        scope: &str,
    ) -> Result<AccountPermit, StoreError> {
        loop {
            if let Some(at) = store
                .provider_retry_deadline(account.into(), scope.into())
                .await?
            {
                clock.wait_until(at).await;
            }
            let permit = self.acquire(account).await?;
            // A preceding account request may have just recorded new guidance.
            // Release both permits before waiting so other work remains possible.
            if store
                .provider_retry_deadline(account.into(), scope.into())
                .await?
                .is_none_or(|at| at <= clock.now_ms())
            {
                return Ok(permit);
            }
            drop(permit);
        }
    }
    pub async fn acquire(&self, account: &str) -> Result<AccountPermit, StoreError> {
        let lane = {
            let mut accounts = self.accounts.lock().await;
            accounts.retain(|_, lane| lane.strong_count() != 0);
            if let Some(lane) = accounts.get(account).and_then(Weak::upgrade) {
                lane
            } else {
                let lane = Arc::new(Mutex::new(()));
                accounts.insert(account.to_owned(), Arc::downgrade(&lane));
                lane
            }
        };
        // Waiting on one account must not consume another account's global slot.
        let sequence = lane.lock_owned().await;
        let active = self
            .active
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        Ok(AccountPermit {
            _active: active,
            _sequence: sequence,
        })
    }
}
