//! SQLite FTS5 full-text search indexing and trigram query engine.

use crate::db::{DatabaseEngine, DatabaseError};
use serde::{Deserialize, Serialize};

/// Search hit result for full-text query matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// Entity ID matching the query.
    pub id: String,
    /// Matching title or subject.
    pub title: String,
    /// Text snippet matching the search term.
    pub snippet: String,
}

/// Full-text search engine managing FTS5 virtual tables and trigram queries.
pub struct SearchEngine<'a> {
    db: &'a DatabaseEngine,
}

impl<'a> SearchEngine<'a> {
    /// Create a new `SearchEngine` bound to a `DatabaseEngine`.
    ///
    /// Unlike earlier versions of this type, `SearchEngine` no longer lazily creates the FTS5
    /// virtual tables on first use: [`DatabaseEngine::migrate`] creates `messages_fts` and
    /// `events_fts` (and backfills `messages_fts` for any pre-existing rows) up front, so every
    /// message or event ever written is searchable as soon as the engine is open, not just
    /// those saved after the first search call.
    pub fn new(db: &'a DatabaseEngine) -> Self {
        Self { db }
    }

    /// Sanitize user search inputs to prevent SQLite FTS5 query operator syntax errors.
    pub fn sanitize_fts5_query(query: &str) -> String {
        let clean = query.trim();
        if clean.is_empty() {
            return String::new();
        }
        let sanitized = clean.replace(['"', '*', ':'], "");
        format!("\"{}\"", sanitized)
    }

    /// Perform a full-text trigram search over email subjects, senders, and body text.
    pub async fn search_messages(&self, query: &str) -> Result<Vec<SearchHit>, DatabaseError> {
        let clean_query = Self::sanitize_fts5_query(query);
        if clean_query.is_empty() {
            return Ok(Vec::new());
        }

        let rows: Vec<(String, String, String)> = sqlx::query_as(
            r#"
            SELECT id, subject, snippet(messages_fts, 3, '<b>', '</b>', '...', 10) as snippet
            FROM messages_fts
            WHERE messages_fts MATCH ?
            ORDER BY rank
            LIMIT 50
            "#,
        )
        .bind(&clean_query)
        .fetch_all(self.db.pool())
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(id, title, snippet)| SearchHit { id, title, snippet })
            .collect())
    }

    /// Perform a full-text trigram search over calendar event summaries and locations.
    pub async fn search_events(&self, query: &str) -> Result<Vec<SearchHit>, DatabaseError> {
        let clean_query = Self::sanitize_fts5_query(query);
        if clean_query.is_empty() {
            return Ok(Vec::new());
        }

        let rows: Vec<(String, String, String)> = sqlx::query_as(
            r#"
            SELECT id, summary, snippet(events_fts, 2, '<b>', '</b>', '...', 10) as snippet
            FROM events_fts
            WHERE events_fts MATCH ?
            ORDER BY rank
            LIMIT 50
            "#,
        )
        .bind(&clean_query)
        .fetch_all(self.db.pool())
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(id, title, snippet)| SearchHit { id, title, snippet })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_email(id: &str, subject: &str, body_plain: &str) -> nuncio_core::model::Email {
        nuncio_core::model::Email {
            id: id.to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "inbox".to_string(),
            remote_id: id.to_string(),
            uid_validity: "1".to_string(),
            subject: subject.to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1_700_000_000,
            read: false,
            body_plain: Some(body_plain.to_string()),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    /// The crux test: proves body search works over the REAL `save_email` write path (no
    /// raw-SQL plaintext injection bypassing encryption), while the stored body column stays
    /// ciphertext. This is what the old `fts5_message_search_and_triggers` test failed to
    /// prove -- it inserted plaintext directly via raw SQL, which trivially "worked" but never
    /// exercised (or caught the bug in) the encrypt-then-index write path.
    #[tokio::test]
    async fn fts5_message_body_search_via_real_save_email_write_path() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let search = SearchEngine::new(&db);

        // Empty search returns empty results, with no setup call needed: migrate() already
        // created the FTS5 tables eagerly.
        assert!(search.search_messages("").await.unwrap().is_empty());

        let email = sample_email(
            "msg-1",
            "Quarterly Financial Meeting",
            "Let's discuss the annual budget revenue forecast",
        );
        db.save_email(&email).await.unwrap();

        // Body search matches a plaintext body term -- proves the FTS index holds real,
        // decrypted-at-write-time plaintext trigrams, not the AES-256-GCM ciphertext that is
        // actually stored in messages.body_plain.
        let hits = search.search_messages("revenue").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "msg-1");
        assert_eq!(hits[0].title, "Quarterly Financial Meeting");

        // Subject search still matches: subject/sender were never encrypted.
        let subject_hits = search.search_messages("Financial").await.unwrap();
        assert_eq!(subject_hits.len(), 1);

        // The body column at rest must remain encrypted ciphertext -- never the plaintext
        // search term, and never equal to the original plaintext body.
        let (stored_body,): (String,) =
            sqlx::query_as("SELECT body_plain FROM messages WHERE id = 'msg-1'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_ne!(stored_body, email.body_plain.clone().unwrap());
        assert!(!stored_body.to_lowercase().contains("revenue"));

        // Re-saving via save_email (an update, not a fresh insert) replaces the FTS row rather
        // than duplicating or leaving a stale entry: the old term no longer matches, the new
        // one does.
        let mut updated = email.clone();
        updated.body_plain = Some("Updated strategy review notes".to_string());
        db.save_email(&updated).await.unwrap();

        assert!(search.search_messages("revenue").await.unwrap().is_empty());
        let new_hits = search.search_messages("strategy").await.unwrap();
        assert_eq!(new_hits.len(), 1);
        assert_eq!(new_hits[0].id, "msg-1");

        // Delete message trigger still cleans up the FTS row (subject/sender/id only -- no
        // ciphertext involved).
        sqlx::query("DELETE FROM messages WHERE id = 'msg-1'")
            .execute(db.pool())
            .await
            .unwrap();

        let deleted_hits = search.search_messages("strategy").await.unwrap();
        assert!(deleted_hits.is_empty());
    }

    /// Proves the backfill half of the fix: a message saved through the real write path,
    /// whose FTS entry is then lost (simulating either a row written before the FTS5 index
    /// existed, or one written by a process that bypassed `save_email`'s explicit indexing),
    /// is NOT searchable until a migration pass runs -- at which point `backfill_message_fts`
    /// decrypts the ciphertext body column with this engine's real storage key and repopulates
    /// the plaintext-derived trigram index, with no search call ever required to trigger it.
    #[tokio::test]
    async fn fts5_backfill_indexes_preexisting_rows_missing_from_the_index() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let email = sample_email(
            "msg-legacy-1",
            "Legacy Roadmap Notes",
            "Confidential quarterly roadmap details",
        );
        db.save_email(&email).await.unwrap();

        // Simulate pre-existing data whose FTS entry is missing (e.g. written before the FTS5
        // index existed) by dropping its messages_fts row directly.
        sqlx::query("DELETE FROM messages_fts WHERE id = 'msg-legacy-1'")
            .execute(db.pool())
            .await
            .unwrap();

        let search = SearchEngine::new(&db);
        assert!(
            search.search_messages("roadmap").await.unwrap().is_empty(),
            "message must be unsearchable once its FTS entry is missing"
        );

        // Re-running migration (idempotent) must backfill the missing row from the encrypted
        // body column -- this is the exact code path a fresh process re-opening this database
        // takes via DatabaseEngine::open/connect_file, with no search call involved.
        db.migrate().await.unwrap();

        let hits = search.search_messages("roadmap").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "msg-legacy-1");
        assert_eq!(hits[0].title, "Legacy Roadmap Notes");
    }

    #[tokio::test]
    async fn fts5_event_search_and_triggers() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let search = SearchEngine::new(&db);

        // Empty search returns empty results
        assert!(search.search_events("").await.unwrap().is_empty());

        // Insert calendar event
        sqlx::query(
            "INSERT INTO calendar_events (id, account_id, calendar_id, summary, start_time, end_time, location)
             VALUES ('evt-1', 'acct-1', 'cal-1', 'Architecture Summit', 1700000000, 1700003600, 'Conference Room B')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        // Search event trigram match
        let hits = search.search_events("Summit").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "evt-1");
        assert_eq!(hits[0].title, "Architecture Summit");
    }

    #[test]
    fn fts5_query_sanitizer_removes_special_operators() {
        assert_eq!(SearchEngine::sanitize_fts5_query(""), "");
        assert_eq!(
            SearchEngine::sanitize_fts5_query("query* with: quotes\""),
            "\"query with quotes\""
        );
    }
}
