//! Real inbound contacts synchronization routine.
//!
//! Mirrors `nunciod::calendar_sync`'s shape for the contacts engine:
//! [`sync_with_backend`] fetches contacts from a [`ContactsBackend`] for a
//! single account, then persists each contact via
//! [`DatabaseEngine::save_contact`].
//!
//! [`sync_carddav_account`] is the production entry point: it resolves a real
//! account's persisted CardDAV address-book collection URL, builds a real
//! [`CardDavClient`] from it plus the credential the caller read from the OS
//! keyring, and funnels through the same [`sync_with_backend`] seam that tests
//! drive with `MockContactsBackend`.
use nuncio_contacts::{CardDavAccountConfig, CardDavClient, ContactsBackend};
use nuncio_store::db::DatabaseError;
use thiserror::Error;

/// Errors that can occur while synchronizing a single account's contacts.
#[derive(Debug, Error)]
pub enum ContactsSyncError {
    /// The contacts backend (CardDAV/mock) reported an error.
    #[error("contacts backend error: {0}")]
    Backend(#[from] nuncio_contacts::CardDavError),
    /// A database persistence error occurred while saving synced contacts.
    #[error("database error: {0}")]
    Store(#[from] DatabaseError),
}

/// Fetch every contact belonging to `account_id` from `backend`, persisting
/// each via [`DatabaseEngine::save_contact`]. Returns the number of contacts
/// processed.
///
/// Pure fetch-and-persist, unit-testable directly with a
/// [`nuncio_contacts::MockContactsBackend`] -- no event-bus or credential
/// logic, mirroring `nunciod::calendar_sync::sync_with_backend`.
pub async fn sync_with_backend(
    db: &nuncio_store::db::DatabaseEngine,
    backend: &dyn ContactsBackend,
    account_id: &str,
) -> Result<usize, ContactsSyncError> {
    tracing::info!(
        account_id = %account_id,
        "contacts sync: dispatching sync for account"
    );
    let contacts = backend.fetch_contacts(account_id).await.inspect_err(|e| {
        tracing::warn!(
            account_id = %account_id,
            "contacts sync: backend fetch failed: {e}"
        );
    })?;
    let fetched = contacts.len();
    let mut synced = 0usize;
    for contact in contacts {
        db.save_contact(&contact).await?;
        synced += 1;
    }
    tracing::info!(
        account_id = %account_id,
        fetched,
        synced,
        "contacts sync: persisted fetched contacts"
    );
    Ok(synced)
}

/// Production entry point: build a real [`CardDavClient`] for `account` from
/// its persisted `collection_url` and the `password` resolved from the OS
/// keyring vault, then fetch-and-persist that account's contacts via
/// [`sync_with_backend`]. Returns the number of contacts synced.
///
/// `account`'s collection URL must be a fully-qualified CardDAV address-book
/// collection URL (validated at `AddAccount` time); the account's
/// `email_address` is used as the DAV basic-auth username. No fabricated data
/// is ever returned -- a transport or persistence failure surfaces as a
/// [`ContactsSyncError`].
pub async fn sync_carddav_account(
    db: &nuncio_store::db::DatabaseEngine,
    account: &nuncio_core::AccountConfig,
    password: &str,
) -> Result<usize, ContactsSyncError> {
    tracing::info!(
        account_id = %account.id,
        "contacts sync: dispatching CardDAV sync for account"
    );
    let client = CardDavClient::new(CardDavAccountConfig {
        account_id: account.id.clone(),
        carddav_url: account.dav_collection_url().unwrap_or_default().to_string(),
        username: account.email_address.clone(),
        auth_token: nuncio_core::redact::Redacted::new(password.to_string()),
    });
    sync_with_backend(db, &client, &account.id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_contacts::{Contact, MockContactsBackend};
    use nuncio_store::db::DatabaseEngine;

    fn mock_contact(id: &str, account_id: &str, display_name: &str) -> Contact {
        let mut contact = Contact::new(display_name, format!("{display_name}@nuncio.mx"));
        contact.id = id.to_string();
        contact.account_id = Some(account_id.to_string());
        contact
    }

    #[tokio::test]
    async fn sync_with_backend_persists_mock_contacts() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");

        let mock = MockContactsBackend::new();
        mock.add_contact(mock_contact("ct-1", "acct-contacts-1", "Alice"));
        mock.add_contact(mock_contact("ct-2", "acct-contacts-1", "Bob"));
        // Belongs to a different account -- must not be returned or persisted
        // for `acct-contacts-1`'s sync.
        mock.add_contact(mock_contact("ct-other", "acct-other", "Carol"));

        let synced = sync_with_backend(&db, &mock, "acct-contacts-1")
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 2);

        let persisted = db
            .list_contacts("acct-contacts-1")
            .await
            .expect("list persisted contacts");
        assert_eq!(persisted.len(), 2);
        let ids: Vec<&str> = persisted.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"ct-1"));
        assert!(ids.contains(&"ct-2"));
    }

    #[tokio::test]
    async fn sync_with_backend_returns_zero_for_empty_backend() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let mock = MockContactsBackend::new();

        let synced = sync_with_backend(&db, &mock, "acct-empty")
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 0);
    }

    #[tokio::test]
    async fn sync_with_backend_surfaces_backend_failure() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let mock = MockContactsBackend::new();
        mock.set_should_fail(true);

        let err = sync_with_backend(&db, &mock, "acct-contacts-1")
            .await
            .expect_err("sync fails");
        assert!(matches!(err, ContactsSyncError::Backend(_)));
        assert!(err.to_string().contains("contacts backend error"));
    }

    /// A contacts sync must emit a followable INFO event carrying the account
    /// id and the persisted count.
    #[test]
    fn sync_with_backend_logs_account_id_and_synced_count() {
        use crate::test_tracing::with_recorder;
        use tracing::Level;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let (recorder, ()) = with_recorder(|| {
            runtime.block_on(async {
                let (db, _dir) = DatabaseEngine::connect_ephemeral()
                    .await
                    .expect("ephemeral db");
                let mock = MockContactsBackend::new();
                mock.add_contact(mock_contact("ct-1", "acct-contacts-1", "Alice"));
                sync_with_backend(&db, &mock, "acct-contacts-1")
                    .await
                    .expect("sync succeeds");
            });
        });

        let events = recorder.events();
        let persisted = events
            .iter()
            .find(|e| e.message() == "contacts sync: persisted fetched contacts")
            .expect("a domain event for the persisted sync must be logged");
        assert_eq!(persisted.level, Level::INFO);
        assert_eq!(
            persisted.fields.get("account_id").map(String::as_str),
            Some("acct-contacts-1")
        );
        assert_eq!(
            persisted.fields.get("synced").map(String::as_str),
            Some("1")
        );
    }
}
