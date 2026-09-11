use crate::store::{ChangeEvent, Store, StoreError};
use std::collections::VecDeque;
use tokio::sync::watch;

pub struct ChangeReader {
    store: Store,
    changed: watch::Receiver<u64>,
    after: u64,
    buffer: VecDeque<ChangeEvent>,
}
impl ChangeReader {
    pub(crate) async fn open(store: Store, after: u64) -> Result<Self, StoreError> {
        let changed = store.revision.clone();
        let page = store.changes_after(after, 100).await?;
        Ok(Self {
            store,
            changed,
            after,
            buffer: page.changes.into(),
        })
    }
    pub async fn next(&mut self) -> Result<ChangeEvent, StoreError> {
        loop {
            if let Some(change) = self.buffer.pop_front() {
                self.after = change.revision;
                return Ok(change);
            }
            // Subscribe before querying, so a commit between the query and wait
            // cannot be lost. The watch value is only a wake-up hint; SQL is truth.
            self.changed.borrow_and_update();
            self.buffer = self
                .store
                .changes_after(self.after, 100)
                .await?
                .changes
                .into();
            if !self.buffer.is_empty() {
                continue;
            }
            self.changed
                .changed()
                .await
                .map_err(|_| StoreError::Unavailable)?;
        }
    }
}
