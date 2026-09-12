use super::{StoreError, StoreOpenOptions};
use rusqlite::{Connection, Transaction};

pub(super) const VERSION: u32 = 23;

pub(super) fn check_version(connection: &Connection) -> Result<u32, StoreError> {
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > VERSION {
        return Err(StoreError::FutureSchema);
    }
    Ok(version)
}

pub(super) fn migrate(
    connection: &mut Connection,
    options: &StoreOpenOptions,
) -> Result<(), StoreError> {
    let version = check_version(connection)?;
    if version == 0 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY NOT NULL);
             INSERT INTO schema_migrations VALUES (1);
             CREATE TABLE store_meta (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 revision INTEGER NOT NULL CHECK (revision >= 0)
             );
             INSERT INTO store_meta VALUES (1, 0);
             CREATE TABLE accounts (
                 id TEXT PRIMARY KEY NOT NULL,
                 provider TEXT NOT NULL CHECK (provider IN ('google', 'imap')),
                 address TEXT NOT NULL
             );
             CREATE TABLE change_log (
                 revision INTEGER PRIMARY KEY NOT NULL,
                 kind TEXT NOT NULL,
                 account_id TEXT REFERENCES accounts(id)
             );
             PRAGMA user_version = 1;",
        )?;
        commit(transaction, 1, options)?;
    }
    if version < 2 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "ALTER TABLE accounts ADD COLUMN subject TEXT;
             ALTER TABLE accounts ADD COLUMN state TEXT NOT NULL DEFAULT 'disconnected'
                 CHECK(state IN ('connected','disconnected','needs_auth'));
             ALTER TABLE accounts ADD COLUMN credential_ref TEXT;
             CREATE UNIQUE INDEX account_provider_subject ON accounts(provider,subject) WHERE subject IS NOT NULL;
             CREATE TABLE credential_cleanup (reference TEXT PRIMARY KEY NOT NULL);
             INSERT INTO schema_migrations VALUES (2);
             PRAGMA user_version = 2;"
        )?;
        commit(transaction, 2, options)?;
    }
    if version < 3 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("mail_schema.sql"))?;
        commit(transaction, 3, options)?;
    }
    if version < 4 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("calendar_schema.sql"))?;
        commit(transaction, 4, options)?;
    }
    if version < 5 {
        let transaction = connection.transaction()?;
        transaction.execute_batch("ALTER TABLE change_log ADD COLUMN resource_id TEXT;
            DELETE FROM change_log WHERE revision<=(SELECT revision-10000 FROM store_meta WHERE singleton=1);
            INSERT INTO schema_migrations VALUES (5); PRAGMA user_version=5;")?;
        commit(transaction, 5, options)?;
    }
    if version < 6 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("schedules.sql"))?;
        commit(transaction, 6, options)?;
    }
    if version < 7 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("draft_schema.sql"))?;
        commit(transaction, 7, options)?;
    }
    if version < 8 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("draft_upload_schema.sql"))?;
        commit(transaction, 8, options)?;
    }
    if version < 9 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("draft_context_schema.sql"))?;
        commit(transaction, 9, options)?;
    }
    if version < 10 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("operation_schema.sql"))?;
        commit(transaction, 10, options)?;
    }
    if version < 11 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("mail_change_schema.sql"))?;
        commit(transaction, 11, options)?;
    }
    if version < 12 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("calendar_change_schema.sql"))?;
        commit(transaction, 12, options)?;
    }
    if version < 13 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("imap_account_schema.sql"))?;
        commit(transaction, 13, options)?;
    }
    if version < 14 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("imap_mailbox_schema.sql"))?;
        commit(transaction, 14, options)?;
    }
    if version < 15 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("imap_transfer_schema.sql"))?;
        commit(transaction, 15, options)?;
    }
    if version < 16 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("imap_trash_schema.sql"))?;
        commit(transaction, 16, options)?;
    }
    if version < 17 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("smtp_schema.sql"))?;
        commit(transaction, 17, options)?;
    }
    if version < 18 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("smtp_floor_schema.sql"))?;
        commit(transaction, 18, options)?;
    }
    if version < 19 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("restore_schema.sql"))?;
        commit(transaction, 19, options)?;
    }
    if version < 20 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("calendar_repair_schema.sql"))?;
        commit(transaction, 20, options)?;
    }
    if version < 21 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("reconciliation_schema.sql"))?;
        commit(transaction, 21, options)?;
    }
    if version < 22 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("restore_jobs_schema.sql"))?;
        commit(transaction, 22, options)?;
    }
    if version < 23 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("account_lifecycle_schema.sql"))?;
        commit(transaction, 23, options)?;
    }
    Ok(())
}

fn commit(
    transaction: Transaction<'_>,
    version: u32,
    options: &StoreOpenOptions,
) -> Result<(), StoreError> {
    #[cfg(not(feature = "test-harness"))]
    let _ = (version, options);
    #[cfg(feature = "test-harness")]
    if let Some(config) = &options.test_config {
        config
            .blocking_checkpoint(&format!("migration-{version}-before-commit"))
            .map_err(|_| StoreError::Unavailable)?;
    }
    transaction.commit()?;
    #[cfg(feature = "test-harness")]
    if let Some(config) = &options.test_config {
        config
            .blocking_checkpoint(&format!("migration-{version}-after-commit"))
            .map_err(|_| StoreError::Unavailable)?;
    }
    Ok(())
}
