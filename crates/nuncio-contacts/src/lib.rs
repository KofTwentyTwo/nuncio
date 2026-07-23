pub mod carddav;
pub mod db;
pub mod models;

pub use carddav::{CardDavAccountConfig, CardDavClient, CardDavError};
pub use db::{ContactsDatabase, ContactsStoreError};
pub use models::{Contact, ContactEmail, ContactGroup, ContactPhone};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_contacts_database_crud_and_fts() {
        let db = ContactsDatabase::in_memory().await.expect("Failed to create in-memory db");

        let mut contact = Contact::new("James Maes", "james.maes@kof22.com");
        contact.organization = Some("KofTwentyTwo".to_string());
        db.save_contact(&contact).await.expect("Failed to save contact");

        let results = db.search_contacts("James").await.expect("Failed to search contacts");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_name, "James Maes");
        assert_eq!(results[0].emails[0].email, "james.maes@kof22.com");

        let harvested = db.harvest_email_address("Alice Smith", "alice@kof22.com").await;
        assert!(harvested.is_ok());

        let results2 = db.search_contacts("alice").await.expect("Failed to search harvested");
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0].display_name, "Alice Smith");
    }

    #[test]
    fn test_vcard_generation() {
        let contact = Contact::new("Bob Builder", "bob@builder.com");
        let vcard = contact.to_vcard();
        assert!(vcard.contains("BEGIN:VCARD"));
        assert!(vcard.contains("FN:Bob Builder"));
        assert!(vcard.contains("EMAIL;TYPE=WORK:bob@builder.com"));
        assert!(vcard.contains("END:VCARD"));
    }
}
