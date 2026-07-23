use crate::models::{Contact, ContactEmail, ContactPhone};
use sqlx::{sqlite::SqlitePool, Row};
use std::path::Path;
use thiserror::Error;
use tracing::info;

#[derive(Error, Debug)]
pub enum ContactsStoreError {
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Contact not found: {0}")]
    NotFound(String),
}

#[derive(Clone)]
pub struct ContactsDatabase {
    pool: SqlitePool,
}

impl ContactsDatabase {
    pub async fn in_memory() -> Result<Self, ContactsStoreError> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    pub async fn open(db_path: impl AsRef<Path>) -> Result<Self, ContactsStoreError> {
        let path_str = db_path.as_ref().to_string_lossy();
        let conn_str = format!("sqlite:{}?mode=rwc", path_str);
        let pool = SqlitePool::connect(&conn_str).await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    pub async fn migrate(&self) -> Result<(), ContactsStoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS contacts (
                id TEXT PRIMARY KEY NOT NULL,
                account_id TEXT,
                display_name TEXT NOT NULL,
                given_name TEXT,
                family_name TEXT,
                organization TEXT,
                job_title TEXT,
                notes TEXT,
                avatar_url TEXT,
                emails_json TEXT NOT NULL DEFAULT '[]',
                phones_json TEXT NOT NULL DEFAULT '[]',
                is_favorite INTEGER NOT NULL DEFAULT 0,
                interaction_count INTEGER NOT NULL DEFAULT 0,
                last_interacted_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS contacts_fts USING fts5(
                id UNINDEXED,
                display_name,
                organization,
                emails_json,
                tokenize='trigram'
            );
            "#
        )
        .execute(&self.pool)
        .await?;

        info!("ContactsDatabase migrations applied successfully.");
        Ok(())
    }

    pub async fn save_contact(&self, contact: &Contact) -> Result<(), ContactsStoreError> {
        let emails_json = serde_json::to_string(&contact.emails).unwrap_or_else(|_| "[]".to_string());
        let phones_json = serde_json::to_string(&contact.phones).unwrap_or_else(|_| "[]".to_string());
        let created_str = contact.created_at.to_rfc3339();
        let updated_str = contact.updated_at.to_rfc3339();
        let last_interacted_str = contact.last_interacted_at.map(|t| t.to_rfc3339());

        sqlx::query(
            r#"
            INSERT INTO contacts (
                id, account_id, display_name, given_name, family_name, organization,
                job_title, notes, avatar_url, emails_json, phones_json, is_favorite,
                interaction_count, last_interacted_at, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                display_name = excluded.display_name,
                organization = excluded.organization,
                emails_json = excluded.emails_json,
                phones_json = excluded.phones_json,
                is_favorite = excluded.is_favorite,
                interaction_count = excluded.interaction_count,
                last_interacted_at = excluded.last_interacted_at,
                updated_at = excluded.updated_at
            "#
        )
        .bind(&contact.id)
        .bind(&contact.account_id)
        .bind(&contact.display_name)
        .bind(&contact.given_name)
        .bind(&contact.family_name)
        .bind(&contact.organization)
        .bind(&contact.job_title)
        .bind(&contact.notes)
        .bind(&contact.avatar_url)
        .bind(&emails_json)
        .bind(&phones_json)
        .bind(if contact.is_favorite { 1 } else { 0 })
        .bind(contact.interaction_count as i64)
        .bind(&last_interacted_str)
        .bind(&created_str)
        .bind(&updated_str)
        .execute(&self.pool)
        .await?;

        // Sync to FTS5 index
        sqlx::query("DELETE FROM contacts_fts WHERE id = ?")
            .bind(&contact.id)
            .execute(&self.pool)
            .await?;

        sqlx::query("INSERT INTO contacts_fts (id, display_name, organization, emails_json) VALUES (?, ?, ?, ?)")
            .bind(&contact.id)
            .bind(&contact.display_name)
            .bind(&contact.organization)
            .bind(&emails_json)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn search_contacts(&self, query: &str) -> Result<Vec<Contact>, ContactsStoreError> {
        let pattern = format!("%{}%", query);
        let rows = sqlx::query(
            r#"
            SELECT id, account_id, display_name, given_name, family_name, organization,
                   job_title, notes, avatar_url, emails_json, phones_json, is_favorite,
                   interaction_count, last_interacted_at, created_at, updated_at
            FROM contacts
            WHERE display_name LIKE ? OR organization LIKE ? OR emails_json LIKE ?
            ORDER BY interaction_count DESC, display_name ASC
            LIMIT 50
            "#
        )
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .fetch_all(&self.pool)
        .await?;

        let mut contacts = Vec::new();
        for row in rows {
            let emails_json: String = row.get("emails_json");
            let phones_json: String = row.get("phones_json");
            let emails: Vec<ContactEmail> = serde_json::from_str(&emails_json).unwrap_or_default();
            let phones: Vec<ContactPhone> = serde_json::from_str(&phones_json).unwrap_or_default();
            let is_favorite_int: i32 = row.get("is_favorite");
            let interaction_cnt: i64 = row.get("interaction_count");

            contacts.push(Contact {
                id: row.get("id"),
                account_id: row.get("account_id"),
                display_name: row.get("display_name"),
                given_name: row.get("given_name"),
                family_name: row.get("family_name"),
                organization: row.get("organization"),
                job_title: row.get("job_title"),
                notes: row.get("notes"),
                avatar_url: row.get("avatar_url"),
                emails,
                phones,
                is_favorite: is_favorite_int != 0,
                interaction_count: interaction_cnt as u64,
                last_interacted_at: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            });
        }

        Ok(contacts)
    }

    pub async fn list_contacts(&self) -> Result<Vec<Contact>, ContactsStoreError> {
        self.search_contacts("").await
    }

    pub async fn harvest_email_address(&self, display_name: &str, email: &str) -> Result<(), ContactsStoreError> {
        if email.trim().is_empty() {
            return Ok(());
        }

        let existing = self.search_contacts(email).await?;
        if existing.is_empty() {
            let name = if display_name.trim().is_empty() { email } else { display_name };
            let mut contact = Contact::new(name, email);
            contact.interaction_count = 1;
            contact.last_interacted_at = Some(chrono::Utc::now());
            self.save_contact(&contact).await?;
        }
        Ok(())
    }
}
