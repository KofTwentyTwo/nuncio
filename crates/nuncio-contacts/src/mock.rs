//! Deterministic mock contacts backend for offline testing and integration verification.

use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::backend::ContactsBackend;
use crate::carddav::CardDavError;
use crate::models::Contact;

/// Thread-safe mock contacts backend for offline testing.
#[derive(Debug, Clone, Default)]
pub struct MockContactsBackend {
    contacts: Arc<Mutex<Vec<Contact>>>,
    should_fail: Arc<Mutex<bool>>,
}

impl MockContactsBackend {
    /// Create a new `MockContactsBackend`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the mock to simulate CardDAV network failure errors.
    pub fn set_should_fail(&self, fail: bool) {
        if let Ok(mut flag) = self.should_fail.lock() {
            *flag = fail;
        }
    }

    /// Add a mock contact to storage.
    pub fn add_contact(&self, contact: Contact) {
        if let Ok(mut guard) = self.contacts.lock() {
            guard.push(contact);
        }
    }

    /// Retrieve contacts belonging to a specific account.
    pub fn list_contacts(&self, account_id: &str) -> Result<Vec<Contact>, CardDavError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| CardDavError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(CardDavError::TransportFailed(
                "simulated CardDAV network failure".to_string(),
            ));
        }

        let guard = self
            .contacts
            .lock()
            .map_err(|e| CardDavError::ParseFailed(e.to_string()))?;
        let matches = guard
            .iter()
            .filter(|c| c.account_id.as_deref() == Some(account_id))
            .cloned()
            .collect();

        Ok(matches)
    }
}

#[async_trait]
impl ContactsBackend for MockContactsBackend {
    async fn fetch_contacts(&self, account_id: &str) -> Result<Vec<Contact>, CardDavError> {
        self.list_contacts(account_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Contact as ModelContact;

    fn sample_contact(account_id: &str, display_name: &str) -> ModelContact {
        let mut contact = ModelContact::new(display_name, format!("{display_name}@nuncio.mx"));
        contact.account_id = Some(account_id.to_string());
        contact
    }

    #[test]
    fn mock_contacts_backend_operations() {
        let mock = MockContactsBackend::new();
        mock.add_contact(sample_contact("acct-1", "Mock Contact"));

        let contacts = mock
            .list_contacts("acct-1")
            .expect("list contacts succeeds");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].display_name, "Mock Contact");

        assert!(mock
            .list_contacts("acct-other")
            .expect("list succeeds")
            .is_empty());

        mock.set_should_fail(true);
        assert!(mock.list_contacts("acct-1").is_err());
    }

    /// Proves `MockContactsBackend` is usable behind the `ContactsBackend` trait object, the
    /// same way the daemon will inject either it or the real CardDAV client.
    #[tokio::test]
    async fn mock_contacts_backend_is_usable_as_a_trait_object() {
        let mock = MockContactsBackend::new();
        mock.add_contact(sample_contact("acct-1", "Trait Object Contact"));

        let backend: Arc<dyn ContactsBackend> = Arc::new(mock);
        let contacts = backend
            .fetch_contacts("acct-1")
            .await
            .expect("fetch contacts succeeds");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].display_name, "Trait Object Contact");
    }
}
