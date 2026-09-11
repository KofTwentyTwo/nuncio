use super::Engine;
use crate::{
    domain::calendar::AgendaWindow,
    store::{ProjectionScope, RepairPreview, StoreError, SyncRun},
    sync_error::SyncError,
};
use serde::Serialize;
#[derive(Serialize)]
pub struct RepairResult {
    #[serde(flatten)]
    pub preview: RepairPreview,
    pub dry_run: bool,
    pub window: Option<AgendaWindow>,
    pub run: Option<SyncRun>,
}
impl Engine {
    pub async fn repair_projection(
        &self,
        account: String,
        scope: ProjectionScope,
        window: Option<AgendaWindow>,
        dry_run: bool,
    ) -> Result<RepairResult, SyncError> {
        let window = match scope {
            ProjectionScope::Mail => {
                if window.is_some() {
                    return Err(StoreError::InvalidInput.into());
                }
                None
            }
            ProjectionScope::Calendar => Some(match window {
                Some(w) => {
                    AgendaWindow::new(&w.from, &w.to).map_err(|_| StoreError::InvalidInput)?
                }
                None => AgendaWindow::rolling(self.accounts.http.clock.now_ms())
                    .map_err(|_| StoreError::InvalidInput)?,
            }),
        };
        let preview = self.store.preview_repair(account.clone(), scope).await?;
        let run = if dry_run {
            None
        } else {
            Some(match scope {
                ProjectionScope::Mail => self.mail.start(account, true, None).await?,
                ProjectionScope::Calendar => {
                    self.calendar
                        .repair(account, window.clone().ok_or(StoreError::InvalidInput)?)
                        .await?
                }
            })
        };
        Ok(RepairResult {
            preview,
            dry_run,
            window,
            run,
        })
    }
}
