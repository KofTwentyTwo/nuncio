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
    pub fn new(db: &'a DatabaseEngine) -> Self {
        Self { db }
    }

    /// Initialize FTS5 virtual tables and sync triggers.
    pub async fn setup_fts_tables(&self) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                id UNINDEXED,
                subject,
                sender,
                body_plain,
                tokenize = 'trigram'
            );

            CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(id, subject, sender, body_plain)
                VALUES (new.id, new.subject, new.sender, COALESCE(new.body_plain, ''));
            END;

            CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                DELETE FROM messages_fts WHERE id = old.id;
            END;

            CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
                DELETE FROM messages_fts WHERE id = old.id;
                INSERT INTO messages_fts(id, subject, sender, body_plain)
                VALUES (new.id, new.subject, new.sender, COALESCE(new.body_plain, ''));
            END;

            CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
                id UNINDEXED,
                summary,
                location,
                tokenize = 'trigram'
            );

            CREATE TRIGGER IF NOT EXISTS events_ai AFTER INSERT ON calendar_events BEGIN
                INSERT INTO events_fts(id, summary, location)
                VALUES (new.id, new.summary, COALESCE(new.location, ''));
            END;

            CREATE TRIGGER IF NOT EXISTS events_ad AFTER DELETE ON calendar_events BEGIN
                DELETE FROM events_fts WHERE id = old.id;
            END;

            CREATE TRIGGER IF NOT EXISTS events_au AFTER UPDATE ON calendar_events BEGIN
                DELETE FROM events_fts WHERE id = old.id;
                INSERT INTO events_fts(id, summary, location)
                VALUES (new.id, new.summary, COALESCE(new.location, ''));
            END;
            "#,
        )
        .execute(self.db.pool())
        .await
        .map_err(DatabaseError::Query)?;

        Ok(())
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

    #[tokio::test]
    async fn fts5_message_search_and_triggers() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let search = SearchEngine::new(&db);
        search.setup_fts_tables().await.unwrap();

        // Empty search returns empty results
        assert!(search.search_messages("").await.unwrap().is_empty());

        // Insert message
        sqlx::query(
            "INSERT INTO messages (id, account_id, folder_id, subject, sender, recipient, received_at, read_flag, body_plain)
             VALUES ('msg-1', 'acct-1', 'inbox', 'Quarterly Financial Meeting', 'alice@nuncio.mx', 'bob@nuncio.mx', 1700000000, 0, 'Discuss budget revenue')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        // Search trigram match
        let hits = search.search_messages("Financial").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "msg-1");
        assert_eq!(hits[0].title, "Quarterly Financial Meeting");

        // Update message trigger
        sqlx::query("UPDATE messages SET subject = 'Updated Strategy Review' WHERE id = 'msg-1'")
            .execute(db.pool())
            .await
            .unwrap();

        let updated_hits = search.search_messages("Strategy").await.unwrap();
        assert_eq!(updated_hits.len(), 1);
        assert_eq!(updated_hits[0].title, "Updated Strategy Review");

        // Delete message trigger
        sqlx::query("DELETE FROM messages WHERE id = 'msg-1'")
            .execute(db.pool())
            .await
            .unwrap();

        let deleted_hits = search.search_messages("Strategy").await.unwrap();
        assert!(deleted_hits.is_empty());
    }

    #[tokio::test]
    async fn fts5_event_search_and_triggers() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let search = SearchEngine::new(&db);
        search.setup_fts_tables().await.unwrap();

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
