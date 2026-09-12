use crate::store::StoreError;
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

pub(crate) struct Coordinator {
    active: Arc<Semaphore>,
    admitted: Arc<Semaphore>,
    accounts: Mutex<BTreeMap<String, Weak<Mutex<()>>>>,
}
pub(crate) struct AccountPermit {
    _active: OwnedSemaphorePermit,
    _admitted: OwnedSemaphorePermit,
    _sequence: OwnedMutexGuard<()>,
}
impl Coordinator {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            active: Arc::new(Semaphore::new(2)),
            admitted: Arc::new(Semaphore::new(64)),
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
        // Bound retained callers before they wait for account sequencing.
        let admitted = self
            .admitted
            .clone()
            .try_acquire_owned()
            .map_err(|_| StoreError::Busy)?;
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
            _admitted: admitted,
            _sequence: sequence,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use futures_util::{stream::FuturesUnordered, StreamExt};
    use std::time::Duration;

    #[tokio::test]
    async fn account_waiters_have_bounded_admission_and_cancellation_releases_capacity() {
        let coordinator = Coordinator::new();
        let first = coordinator.acquire("same-account").await.unwrap();
        let mut waiting = FuturesUnordered::new();
        for _ in 0..64 {
            waiting.push(coordinator.acquire("same-account"));
        }
        let rejected = tokio::time::timeout(Duration::from_secs(1), waiting.next()).await;
        assert!(rejected.is_ok(),"the 65th admitted account request must fail instead of joining an unbounded wait queue");
        assert!(matches!(rejected.unwrap(), Some(Err(StoreError::Busy))));
        drop(waiting);
        let other =
            tokio::time::timeout(Duration::from_secs(1), coordinator.acquire("other-account"))
                .await
                .unwrap()
                .unwrap();
        let mut third = Box::pin(coordinator.acquire("third-account"));
        assert!(
            futures_util::poll!(&mut third).is_pending(),
            "only two accounts may be active"
        );
        drop(other);
        let third = tokio::time::timeout(Duration::from_secs(1), third)
            .await
            .unwrap()
            .unwrap();
        drop(third);
        drop(first);
        let fresh =
            tokio::time::timeout(Duration::from_secs(1), coordinator.acquire("same-account"))
                .await
                .unwrap()
                .unwrap();
        drop(fresh);
    }
}
