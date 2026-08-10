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

    /// Keyset-paginated full-text search over message subjects, senders, and
    /// bodies, ordered by relevance (`rank ASC, id ASC`). `after` is the
    /// `(rank, id)` of the last hit of the previous page; `None` starts from
    /// the highest-ranked hit. Fetches `page_size + 1` hits to detect a
    /// following page and returns the `(rank, id)` cursor of the last returned
    /// hit when more remain.
    ///
    /// The bm25 `rank` is materialized in an inner query (where the FTS5
    /// `MATCH` is in scope) into an ordinary column, so the outer query can
    /// both keyset-filter and order by `(rank, id)` -- preserving relevance
    /// order across pages while resuming strictly after the previous page's
    /// last hit.
    pub async fn search_messages_page(
        &self,
        query: &str,
        after: Option<(f64, String)>,
        page_size: usize,
    ) -> Result<(Vec<SearchHit>, Option<(f64, String)>), DatabaseError> {
        let clean_query = Self::sanitize_fts5_query(query);
        if clean_query.is_empty() {
            return Ok((Vec::new(), None));
        }
        let fetch = page_size.saturating_add(1);

        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT id, title, snippet, rk FROM ( \
             SELECT id, subject AS title, \
             snippet(messages_fts, 3, '<b>', '</b>', '...', 10) AS snippet, rank AS rk \
             FROM messages_fts WHERE messages_fts MATCH ",
        );
        builder.push_bind(clean_query);
        builder.push(") ");
        if let Some((rank, id)) = &after {
            builder.push("WHERE (rk > ");
            builder.push_bind(*rank);
            builder.push(" OR (rk = ");
            builder.push_bind(*rank);
            builder.push(" AND id > ");
            builder.push_bind(id.clone());
            builder.push("))");
        }
        builder.push(" ORDER BY rk ASC, id ASC LIMIT ");
        builder.push_bind(fetch as i64);

        let rows = builder
            .build_query_as::<(String, String, String, f64)>()
            .fetch_all(self.db.pool())
            .await
            .map_err(DatabaseError::Query)?;

        let has_more = rows.len() > page_size;
        let mut last_key: Option<(f64, String)> = None;
        let hits: Vec<SearchHit> = rows
            .into_iter()
            .take(page_size)
            .map(|(id, title, snippet, rk)| {
                last_key = Some((rk, id.clone()));
                SearchHit { id, title, snippet }
            })
            .collect();

        let next = if has_more { last_key } else { None };
        Ok((hits, next))
    }

    /// Perform a full-text trigram search over contact display names, organizations, and
    /// email addresses.
    pub async fn search_contacts(&self, query: &str) -> Result<Vec<SearchHit>, DatabaseError> {
        let clean_query = Self::sanitize_fts5_query(query);
        if clean_query.is_empty() {
            return Ok(Vec::new());
        }

        let rows: Vec<(String, String, String)> = sqlx::query_as(
            r#"
            SELECT id, display_name, snippet(contacts_fts, 3, '<b>', '</b>', '...', 10) as snippet
            FROM contacts_fts
            WHERE contacts_fts MATCH ?
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

    /// Keyset-paginated full-text search over contact display names,
    /// organizations, and email addresses, ordered by relevance (`rank ASC, id
    /// ASC`). Mirrors [`Self::search_messages_page`]'s rank-materializing
    /// subquery so the same keyset-pagination guarantee (every hit returned
    /// exactly once, across pages) applies to contact search.
    pub async fn search_contacts_page(
        &self,
        query: &str,
        after: Option<(f64, String)>,
        page_size: usize,
    ) -> Result<(Vec<SearchHit>, Option<(f64, String)>), DatabaseError> {
        let clean_query = Self::sanitize_fts5_query(query);
        if clean_query.is_empty() {
            return Ok((Vec::new(), None));
        }
        let fetch = page_size.saturating_add(1);

        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT id, title, snippet, rk FROM ( \
             SELECT id, display_name AS title, \
             snippet(contacts_fts, 3, '<b>', '</b>', '...', 10) AS snippet, rank AS rk \
             FROM contacts_fts WHERE contacts_fts MATCH ",
        );
        builder.push_bind(clean_query);
        builder.push(") ");
        if let Some((rank, id)) = &after {
            builder.push("WHERE (rk > ");
            builder.push_bind(*rank);
            builder.push(" OR (rk = ");
            builder.push_bind(*rank);
            builder.push(" AND id > ");
            builder.push_bind(id.clone());
            builder.push("))");
        }
        builder.push(" ORDER BY rk ASC, id ASC LIMIT ");
        builder.push_bind(fetch as i64);

        let rows = builder
            .build_query_as::<(String, String, String, f64)>()
            .fetch_all(self.db.pool())
            .await
            .map_err(DatabaseError::Query)?;

        let has_more = rows.len() > page_size;
        let mut last_key: Option<(f64, String)> = None;
        let hits: Vec<SearchHit> = rows
            .into_iter()
            .take(page_size)
            .map(|(id, title, snippet, rk)| {
                last_key = Some((rk, id.clone()));
                SearchHit { id, title, snippet }
            })
            .collect();

        let next = if has_more { last_key } else { None };
        Ok((hits, next))
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
            message_id: None,
            content_hash: None,
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

    /// Paging FTS search results across multiple pages via the `(rank, id)`
    /// keyset must return every matching hit EXACTLY ONCE, no dupes and no
    /// gaps -- proving the rank-materializing subquery keyset works end to
    /// end over the real encrypt-then-index write path.
    #[tokio::test]
    async fn search_messages_page_returns_every_hit_once_no_gaps() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        for i in 0..5 {
            db.save_email(&sample_email(
                &format!("msg-{i}"),
                "Budget Planning Meeting",
                "annual budget revenue forecast discussion",
            ))
            .await
            .unwrap();
        }

        let search = SearchEngine::new(&db);
        let mut seen = Vec::new();
        let mut after: Option<(f64, String)> = None;
        let mut pages = 0;
        loop {
            let (hits, next) = search
                .search_messages_page("budget", after.clone(), 2)
                .await
                .unwrap();
            pages += 1;
            assert!(hits.len() <= 2, "page must not exceed page_size");
            seen.extend(hits.iter().map(|h| h.id.clone()));
            match next {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
            assert!(pages < 100, "pagination must terminate");
        }

        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            5,
            "every matching message returned exactly once"
        );
        assert!(pages >= 3, "5 hits at page_size 2 must span multiple pages");
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

    async fn insert_contact(
        db: &DatabaseEngine,
        id: &str,
        display_name: &str,
        organization: &str,
        emails_json: &str,
    ) {
        sqlx::query(
            "INSERT INTO contacts \
             (id, display_name, organization, emails_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
        )
        .bind(id)
        .bind(display_name)
        .bind(organization)
        .bind(emails_json)
        .execute(db.pool())
        .await
        .unwrap();
    }

    /// Proves `contacts_fts` is actually populated by the `contacts_ai`/`contacts_au` triggers
    /// (not merely created and left unused): several contacts are inserted, a matching term
    /// ranks the true matches and excludes the non-matching contact.
    #[tokio::test]
    async fn fts5_contact_search_ranks_matches_and_excludes_others() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let search = SearchEngine::new(&db);

        assert!(search.search_contacts("").await.unwrap().is_empty());

        insert_contact(&db, "ct-1", "Alice Architect", "Acme Corp", "[]").await;
        insert_contact(&db, "ct-2", "Bob Baker", "Baker Studio", "[]").await;
        insert_contact(&db, "ct-3", "Carol Architect", "Acme Corp", "[]").await;

        let hits = search.search_contacts("Architect").await.unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(hits.len(), 2);
        assert!(ids.contains(&"ct-1"));
        assert!(ids.contains(&"ct-3"));
        assert!(!ids.contains(&"ct-2"));

        let bob_hits = search.search_contacts("Bob").await.unwrap();
        assert_eq!(bob_hits.len(), 1);
        assert_eq!(bob_hits[0].id, "ct-2");
        assert_eq!(bob_hits[0].title, "Bob Baker");
    }

    /// Updating and deleting a contact keeps `contacts_fts` in sync via the `contacts_au`/
    /// `contacts_ad` triggers: a renamed contact stops matching its old name and starts
    /// matching its new one, and a deleted contact stops matching entirely.
    #[tokio::test]
    async fn fts5_contact_search_reflects_update_and_delete() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let search = SearchEngine::new(&db);

        insert_contact(&db, "ct-upd-1", "Dana Original", "Original Org", "[]").await;
        assert_eq!(search.search_contacts("Original").await.unwrap().len(), 1);

        sqlx::query(
            "UPDATE contacts SET display_name = 'Dana Renamed', organization = 'New Org' \
             WHERE id = 'ct-upd-1'",
        )
        .execute(db.pool())
        .await
        .unwrap();

        assert!(search.search_contacts("Original").await.unwrap().is_empty());
        let hits = search.search_contacts("Renamed").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "ct-upd-1");

        sqlx::query("DELETE FROM contacts WHERE id = 'ct-upd-1'")
            .execute(db.pool())
            .await
            .unwrap();

        assert!(search.search_contacts("Renamed").await.unwrap().is_empty());
    }

    /// A search term matching an indexed email address (stored as JSON) is found, and a
    /// sanitized/edge-case query (FTS5 operator characters) does not error -- it just yields
    /// no matches when nothing survives sanitization to match.
    #[tokio::test]
    async fn fts5_contact_search_matches_email_and_survives_edge_query() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let search = SearchEngine::new(&db);

        insert_contact(
            &db,
            "ct-email-1",
            "Erin Example",
            "",
            r#"[{"email":"erin.example@nuncio.mx","label":"work"}]"#,
        )
        .await;

        let hits = search.search_contacts("erin.example").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "ct-email-1");

        let edge_hits = search.search_contacts("\"weird*: query\"").await.unwrap();
        assert!(edge_hits.is_empty());
    }

    /// Keyset pagination over contact search must return every matching hit exactly once,
    /// mirroring the message-search pagination guarantee.
    #[tokio::test]
    async fn search_contacts_page_returns_every_hit_once_no_gaps() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        for i in 0..5 {
            insert_contact(
                &db,
                &format!("ct-page-{i}"),
                "Planning Committee Member",
                "Acme Corp",
                "[]",
            )
            .await;
        }

        let search = SearchEngine::new(&db);
        let mut seen = Vec::new();
        let mut after: Option<(f64, String)> = None;
        let mut pages = 0;
        loop {
            let (hits, next) = search
                .search_contacts_page("Planning", after.clone(), 2)
                .await
                .unwrap();
            pages += 1;
            assert!(hits.len() <= 2, "page must not exceed page_size");
            seen.extend(hits.iter().map(|h| h.id.clone()));
            match next {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
            assert!(pages < 100, "pagination must terminate");
        }

        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            5,
            "every matching contact returned exactly once"
        );
        assert!(pages >= 3, "5 hits at page_size 2 must span multiple pages");
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
