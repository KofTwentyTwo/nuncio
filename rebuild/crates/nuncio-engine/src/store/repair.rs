use super::{Store, StoreError};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionScope {
    Mail,
    Calendar,
}
#[derive(Default, Serialize)]
pub struct ProjectionCounts {
    pub messages: u64,
    pub collections: u64,
    pub attachments: u64,
    pub search_entries: u64,
    pub calendars: u64,
    pub events: u64,
    pub occurrences: u64,
}
#[derive(Serialize)]
pub struct PreservedStateCounts {
    pub drafts: u64,
    pub draft_attachments: u64,
    pub operations: u64,
    pub attempts: u64,
    pub receipts: u64,
}
#[derive(Serialize)]
pub struct RepairPreview {
    pub account_id: String,
    pub scope: ProjectionScope,
    pub revision: u64,
    pub projection: ProjectionCounts,
    pub preserved: PreservedStateCounts,
}
impl Store {
    pub async fn preview_repair(
        &self,
        account: String,
        scope: ProjectionScope,
    ) -> Result<RepairPreview, StoreError> {
        self.execute(move |c| {
            let tx = c.transaction()?;
            let provider: String = tx
                .query_row(
                    "SELECT provider FROM accounts WHERE id=?1",
                    [&account],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(StoreError::NotFound)?;
            if !matches!(provider.as_str(), "google" | "imap")
                || (matches!(scope, ProjectionScope::Calendar) && provider != "google")
            {
                return Err(StoreError::InvalidInput);
            }
            super::backup::check_integrity(&tx)?;
            let count = |table: &str| -> Result<u64, StoreError> {
                // The table names below are constants, never request/provider data.
                let n: i64 = tx.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE account_id=?1"),
                    params![account],
                    |r| r.get(0),
                )?;
                u64::try_from(n).map_err(|_| StoreError::KeyOrCorrupt)
            };
            let mut projection = ProjectionCounts::default();
            match scope {
                ProjectionScope::Mail => {
                    projection.messages = count("messages")?;
                    projection.collections = count("collections")?;
                    projection.attachments = count("attachments")?;
                    projection.search_entries = count("message_search")?;
                }
                ProjectionScope::Calendar => {
                    projection.calendars = count("calendars")?;
                    projection.events = count("calendar_objects")?;
                    projection.occurrences = count("calendar_occurrences")?;
                }
            }
            let preserved = PreservedStateCounts {
                drafts: count("drafts")?,
                draft_attachments: count("draft_attachments")?,
                operations: count("operations")?,
                attempts: count("operation_attempts")?,
                receipts: count("operation_receipts")?,
            };
            let revision = super::worker::status(&tx)?.revision;
            tx.commit()?;
            Ok(RepairPreview {
                account_id: account,
                scope,
                revision,
                projection,
                preserved,
            })
        })
        .await
    }
}
